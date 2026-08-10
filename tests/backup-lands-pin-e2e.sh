#!/usr/bin/env bash
#
# Regression pin for "does the backup actually LAND, and does the panel tell the
# truth about it?" — the six defects of s289.
#
# Every one of them had the same shape: a green check over a broken thing. s288
# fixed the credential path and proved Test Connection, and stopped there; none of
# these is reachable by reading Test Connection's result, and four of the six make
# the panel state something FALSE about data protection.
#
#   A  The panel capped every agent call at 60s while the agent budgets 600s for
#      the same upload, so an off-site copy that took longer than a minute (a 12MB
#      archive on a slow uplink was enough to measure) had the panel give up while
#      curl/scp kept running. The bytes landed; the panel retried the whole file
#      twice more, recorded it local-only, tripped the per-run destination breaker
#      so every LATER site/database/volume skipped off-siting too, and raised an
#      incident saying the backups "exist only on this server".
#
#   B  `backup_scheduler` computed its upload result into a discarded `_`-prefixed
#      binding and inserted five columns, omitting `uploaded` and `destination_id`
#      — the two the migration added for exactly this. So every per-site scheduled
#      backup that DID go off-site was filed as local-only. Measured on a live box:
#      the SFTP copy sat at the destination while its row read uploaded=f.
#
#   C  Nothing installed `sshpass`, which is the only path a password-authenticated
#      SFTP destination can take — and password is the mode the form offers first.
#      s288 measured SFTP as working only because the test rig had apt-installed
#      sshpass itself, so the product's own gap was invisible to the test.
#
#   D  `--fail` fails on 4xx/5xx and says nothing about 3xx, and without `-L` curl
#      does not follow a redirect: it transfers nothing and EXITS 0. A bucket in
#      the wrong region answers 301, so upload_s3 returned Ok, the agent answered
#      success, and the panel lit the "remote" badge for an object that was never
#      written. Measured: "1 successes, 0 failures, 0 not uploaded off-site"
#      against an empty bucket.
#
#   E  Nothing created the SFTP remote directory and `test_sftp` never touched
#      `remote_path`, so Test passed while every scp failed "No such file or
#      directory". The default is /backups, which exists on almost no server.
#
#   F  S3 Test HEADed the bucket ROOT while upload PUTs into the PREFIX — different
#      permission, different path. Read-only and prefix-scoped keys both read green.
#
# and the one s339 added:
#
#   G  Both SFTP operations were SSH **exec** requests (`ssh host "mkdir -p ..."`,
#      then `scp`). `ForceCommand internal-sftp` — OpenSSH's own documented way to
#      run a chrooted, SFTP-only account — replaces the requested command with the
#      SFTP subsystem, so the mkdir was discarded and no such destination could
#      complete a backup (issue #102). The refusal arrives on STDOUT, so an error
#      built from stderr alone showed the operator nothing but ssh's known-hosts
#      banner. scp had to go too, not just move: OpenSSH 9.0 switched it to the
#      SFTP protocol, so a modern box transfers fine while RHEL 8 and Ubuntu 20.04
#      still speak legacy `scp -t` and would keep failing after a mkdir-only fix.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

EXEC=panel/backend/src/services/backup_policy_executor.rs
SCHED=panel/backend/src/services/backup_scheduler.rs
RB=panel/agent/src/services/remote_backup.rs
RBR=panel/agent/src/routes/remote_backup.rs
SETUP=scripts/setup.sh
for f in "$EXEC" "$SCHED" "$RB" "$RBR" "$SETUP"; do
  [ -f "$f" ] || { echo "missing $f"; exit 1; }
done

# Strip // line comments so a prose mention of a flag cannot satisfy an arm.
# This pin's whole subject is code that LOOKS right, and its own header names
# every string it greps for — without this, the header would pass the suite.
# (Deliberately not stripping /* */: none of these files use block comments, and
# a naive block stripper has already swallowed a whole file here once.)
strip() { sed 's://.*::' "$1"; }

SRC_EXEC=$(strip "$EXEC")
SRC_SCHED=$(strip "$SCHED")
SRC_RB=$(strip "$RB")
SRC_RBR=$(strip "$RBR")

# Match against text already held in a variable, via a HERE-STRING — never a
# pipeline.
#
# `code "$f" | grep -q ...` is the obvious spelling and it is wrong here:
# `grep -q` closes the pipe the moment it matches, the upstream `sed` dies of
# SIGPIPE with status 141, and `set -o pipefail` then reports the whole pipeline
# as FAILED. Whether an arm went green depended on whether sed had finished
# writing before grep bailed — the same source scored differently run to run, and
# the first draft of this suite reported a real fix as missing for exactly that
# reason. A here-string is a single command, so there is no upstream to kill.
has()  { grep -q  -- "$2" <<< "$1"; }
hasE() { grep -qE -- "$2" <<< "$1"; }
# Body of one Rust fn, so an arm cannot be satisfied by a sibling function.
fnbody() { awk "/pub async fn $2\(/,/^}/" <<< "$1"; }

echo
echo "§1 the upload call sites outlive the agent's own upload budget (A)"

# The agent's own budget, read from the upload functions rather than from a bare
# `from_secs(600)` literal — the refactor that fixed D moved the timeout into the
# shared runners' argument lists, and an arm pinned to the old spelling reports a
# problem with the code when the problem is with the arm.
if hasE "$(fnbody "$SRC_RB" upload_s3)" '^ +600,' && hasE "$(fnbody "$SRC_RB" upload_sftp)" '600,'; then
  ok "the agent still budgets 600s for both uploads"
else
  bad "the agent's upload budget moved — this pin's numbers need rechecking"
fi

for f in "$EXEC" "$SCHED"; do
  n=$(basename "$f")
  src=$(strip "$f")
  if has "$src" 'post_long("/backups/upload"'; then
    ok "$n uploads via post_long, not the 60s post"
  else
    bad "$n uses plain post for /backups/upload — 60s cap over a 600s operation"
  fi
  # Pull the digits out of the argument list. Anchoring on `$` matched nothing,
  # because the captured text ends in `)` — an arm that silently extracts an
  # empty string reports "none" and looks like a real failure of the code.
  t=$(grep -oE 'post_long\("/backups/upload", Some\([a-z_]+\.clone\(\)\), [0-9]+' <<< "$src" | grep -oE '[0-9]+' | tail -1)
  if [ -n "$t" ] && [ "$t" -gt 600 ]; then
    ok "$n allows ${t}s — longer than the agent's 600s, so the agent errors first"
  else
    bad "$n passes '${t:-none}' — must exceed the agent's 600s budget"
  fi
done

echo
echo "§2 a scheduled backup records WHERE its bytes went (B)"

if has "$SRC_SCHED" 'INSERT INTO backups (site_id, filename, size_bytes, databases_included, databases_expected, uploaded, destination_id)'; then
  ok "the scheduler's INSERT carries uploaded + destination_id"
else
  bad "the scheduler's INSERT omits uploaded/destination_id — the badge cannot light"
fi

if hasE "$SRC_SCHED" 'let _uploaded_remote'; then
  bad "the upload result is bound to a discarded _-prefixed binding again"
else
  ok "the upload result is bound to a name that is actually read"
fi

if has "$SRC_SCHED" 'bind(if uploaded_remote { row.dest_id } else { None })'; then
  ok "destination_id is bound from the row that drove the upload"
else
  bad "destination_id is not bound from the schedule's own destination"
fi

if has "$(grep -B2 'dest_id: Option<Uuid>' <<< "$SRC_SCHED")" 'allow(dead_code)'; then
  bad "dest_id is #[allow(dead_code)] again — the annotation hides the gap"
else
  ok "dest_id carries no dead_code annotation"
fi

echo
echo "§3 no S3 call treats a redirect as a delivered upload (D)"

# --fail is the flag that made a 3xx look like success. It must be gone from
# every S3 invocation, not just the one that was investigated.
if has "$SRC_RB" '"--fail"'; then
  bad "an S3 curl invocation still passes --fail — 3xx will read as success"
else
  ok "no S3 curl invocation passes --fail"
fi

if has "$SRC_RB" 'fn s3_ok' && has "$SRC_RB" '(200..300).contains'; then
  ok "success is defined as an explicit 2xx"
else
  bad "there is no explicit 2xx check — status is being inferred from exit code"
fi

# All four S3 operations must go through the one runner. Fixing the copy under
# investigation and leaving its siblings is a mistake this file has shipped.
for fn in upload_s3 test_s3 list_s3 delete_s3; do
  if has "$(fnbody "$SRC_RB" "$fn")" 's3_curl('; then
    ok "$fn goes through the shared s3_curl runner"
  else
    bad "$fn builds its own curl call — the 3xx bug can return there"
  fi
done

# Following redirects is NOT the fix: a SigV4 signature is bound to the host and
# path it was signed for, and curl downgrades PUT to GET on 301/302.
if hasE "$SRC_RB" '"-L"|--location'; then
  bad "an S3 call follows redirects — the SigV4 signature will not match the new host"
else
  ok "no S3 call follows redirects"
fi

echo
echo "§4 Test Connection exercises what an upload needs (E, F)"

if has "$SRC_RBR" 'test_s3(bucket, region, endpoint, access_key, secret_key, prefix)'; then
  ok "the S3 test is handed the same prefix the upload writes to"
else
  bad "the S3 test does not receive path_prefix — it probes a different location"
fi

S3TEST=$(fnbody "$SRC_RB" test_s3)
if has "$S3TEST" '"PUT"'; then
  ok "the S3 test performs a PUT, the operation an upload performs"
else
  bad "the S3 test does not write — a read-only key will pass it"
fi
if has "$S3TEST" '"DELETE"'; then
  ok "the S3 test removes its own probe object"
else
  bad "the S3 test leaves its probe behind at the destination"
fi
if has "$S3TEST" '"-I"'; then
  bad "the S3 test still HEADs — that is the check that could not see a 301"
else
  ok "the S3 test no longer relies on HEAD"
fi

UPSFTP=$(fnbody "$SRC_RB" upload_sftp)
SFTPTEST=$(fnbody "$SRC_RB" test_sftp)

if has "$UPSFTP" 'sftp_mkdir_script'; then
  ok "the SFTP upload creates its remote directory"
else
  bad "nothing creates the SFTP remote directory — a transfer will not"
fi

if has "$(grep -A8 'test_sftp(' <<< "$SRC_RBR")" 'remote_path'; then
  ok "the SFTP test is handed remote_path"
else
  bad "the SFTP test never receives remote_path — it cannot check the directory"
fi

if has "$SFTPTEST" 'sftp_mkdir_script'; then
  ok "the SFTP test creates and probes the directory an upload will use"
else
  bad "the SFTP test does not touch remote_path — green while every upload fails"
fi
if has "$SFTPTEST" '"exit".into()'; then
  bad "the SFTP test is back to connecting and exiting — it proves only auth"
else
  ok "the SFTP test does more than authenticate"
fi

echo
echo "§4b the transport survives a server that only allows SFTP (G)"

# `ForceCommand internal-sftp` is OpenSSH's own documented way to run a chrooted,
# SFTP-only account, and it REPLACES whatever command an exec request carried. Both
# operations used to be exec requests, so the mkdir was silently discarded and no
# such destination could ever complete a backup (issue #102).
for pair in "upload_sftp:$UPSFTP" "test_sftp:$SFTPTEST"; do
  fn=${pair%%:*}; body=${pair#*:}
  if hasE "$body" 'run_sftp\("(ssh|scp)"'; then
    bad "$fn issues an SSH exec request — ForceCommand internal-sftp discards it"
  else
    ok "$fn does not depend on an SSH exec request"
  fi
  if has "$body" '"-b"'; then
    ok "$fn drives the SFTP subsystem in batch mode"
  else
    bad "$fn no longer runs a batch script — the transport moved"
  fi
  # A path reaching a batch line unquoted does not fail: sftp splits it and writes
  # to the wrong place under the source filename, exit 0.
  if has "$body" 'sftp_quote'; then
    ok "$fn quotes the paths it puts in a batch line"
  else
    bad "$fn passes an unquoted path — a space silently redirects the transfer"
  fi
  if has "$body" 'reject_unquotable_path'; then
    ok "$fn refuses a remote path carrying a line break"
  else
    bad "$fn accepts a newline in remote_path — that appends a second sftp command"
  fi
done

# scp is not merely unused, it must stay gone: OpenSSH < 9.0 speaks legacy `scp -t`,
# which is an exec request and dies the same way. A modern dev box hides this.
if has "$SRC_RB" '"scp"'; then
  bad "scp is back — on OpenSSH < 9.0 it is an exec request and cannot survive ForceCommand"
else
  ok "no scp invocation remains"
fi

# The refusal arrives on STDOUT; an error built from stderr alone showed the
# operator ssh's known-hosts banner as the whole cause of a failed backup.
if has "$SRC_RB" 'fn command_detail'; then
  ok "a helper exists to report both output streams"
else
  bad "nothing joins stdout into the error — a refusal on stdout reads as no error at all"
fi
for fn in upload_sftp test_sftp; do
  if has "$(fnbody "$SRC_RB" "$fn")" 'command_detail'; then
    ok "$fn reports both streams to the operator"
  else
    bad "$fn reports only one stream — the diagnosis is on the other one"
  fi
done

echo
echo "§5 one ssh option builder, not two kept in step by hand (E)"

builders=$(grep -c 'StrictHostKeyChecking=accept-new' <<< "$SRC_RB")
if [ "$builders" -eq 1 ]; then
  ok "the ssh options are built in exactly one place"
else
  bad "$builders ssh option lists — they drifted once already (ConnectTimeout)"
fi

if has "$SRC_RB" 'fn sftp_opts'; then
  ok "the shared builder exists"
else
  bad "there is no shared ssh option builder"
fi

# BatchMode disables password auth outright; it must stay conditional on key auth.
if has "$(grep -A3 'if key_path.is_some() || password.is_none()' <<< "$SRC_RB")" 'BatchMode=yes'; then
  ok "BatchMode is still applied only when authenticating by key"
else
  bad "BatchMode is unconditional again — it disables the auth sshpass supplies"
fi

# The same trap has a second entrance: `sftp -b` turns BatchMode on BY ITSELF, so
# the password branch has to switch it back off or every password-authenticated
# destination dies at "Permission denied" exactly as it did before s288.
if has "$SRC_RB" 'BatchMode=no'; then
  ok "the password branch disables the BatchMode that -b would impose"
else
  bad "nothing says BatchMode=no — sftp -b re-enables it and password auth dies"
fi

echo
echo "§6 the SFTP upload path's binaries are installed (C)"

SETUP_SRC=$(cat "$SETUP")
if hasE "$SETUP_SRC" 'pkg_install sshpass'; then
  ok "setup.sh installs sshpass"
else
  bad "nothing installs sshpass — password SFTP destinations cannot work"
fi

if hasE "$SETUP_SRC" 'pkg_install .*openssh-client'; then
  ok "setup.sh declares the ssh client"
else
  bad "openssh-client/openssh-clients is an undeclared runtime dependency"
fi

# It must NOT be fatal: sshpass comes from EPEL on RHEL-family, EPEL enablement
# is itself best-effort, and this script runs under `set -e`. Making it mandatory
# would turn "no EPEL" into a failed install for everyone.
if hasE "$SETUP_SRC" 'if ! run "Installing sshpass'; then
  ok "the sshpass install is best-effort, so it cannot abort an install"
else
  bad "the sshpass install is not guarded — a missing package now aborts setup"
fi

# And when it is genuinely absent, the operator must be told which binary.
if has "$SRC_RB" 'sshpass is not installed on this server'; then
  ok "the agent names sshpass and the remedy when it is missing"
else
  bad "a missing sshpass still surfaces as a bare os error 2"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
