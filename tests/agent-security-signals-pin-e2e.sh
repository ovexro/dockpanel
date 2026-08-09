#!/usr/bin/env bash
# agent-security-signals-pin-e2e.sh — s330 / v2.88.0
#
# THE SENTENCE THIS SUITE EXISTS TO PROVE, written out so a mutation can be aimed
# at it rather than at whatever the arms happen to catch:
#
#   "The agent's security controls must not answer with silence: a backup of the
#    panel database is readable only by root, an input line is judged the same
#    whatever WebSocket frame shape carried it, a REFUSED command is reported,
#    a failed privilege drop never reaches execv, and 'no SSH keys' means the
#    agent LOOKED and found none."
#
# THE DEFECTS. Four, all realized, all measured on the demo box at v2.87.0:
#
#  1. `/var/backups/dockpanel` was 0755 and every writer's redirect left the dumps
#     0644. 17 of 17 files were readable as www-data — the uid EVERY hosted site's
#     PHP runs as — and the newest was a full pg_dump carrying `servers.agent_token`
#     in cleartext. That token is the Bearer credential for every agent endpoint
#     AND the HMAC key the agent verifies its own root-PTY tickets with. A web
#     shell on any hosted site was therefore root on the panel host and on every
#     server it manages.
#  2. Terminal input was reconstructed and classified inside the `{"type":"input"}`
#     arm only. The raw-text arm ran the BLOCK half and only for site terminals,
#     so a scripted client — the only kind that sends raw frames, since Terminal.tsx
#     sends JSON at all seven senders — produced ZERO intrusion signal, on a ROOT
#     shell. Measured: JSON frame -> 1 event, raw frame -> 0, same command.
#  3. A REFUSED command `break`s before the classifier, so the strongest thing the
#     product can observe reached nothing but the agent's own journal.
#  4. `list_ssh_keys` read /root/.ssh/authorized_keys with an in-process
#     `read_to_string(...).unwrap_or_default()` while the unit's `ProtectHome=yes`
#     binds an inaccessible dir over /root. It answered 200 with an EMPTY LIST on a
#     box with three live root keys — a false all-clear, on every install, since the
#     handlers landed (2026-03-15) the day after ProtectHome did (2026-03-13).
#
# So the mutations this suite must survive are RE-SILENCING mutations — put the
# classifier back behind one frame shape, let a refusal fall through, discard a
# read error, loosen a mode — not deletions. Every arm below was mutation-tested
# to redden ALONE, and every arm was run against v2.87.0 in a git worktree and
# required to be RED there.
#
#   §A  a backup of the panel database is root-only
#   §B  input is judged the same whatever frame shape carried it
#   §C  a refusal is reported, and does NOT arm the auto-lockdown
#   §D  the root shell's environment and privilege drop survive to execv
#   §E  "no SSH keys" means the agent looked
#
# Pure source analysis: no box, no network, no build.
#
# NO PIPES INTO `grep -q`. Under `set -o pipefail` grep -q closes the pipe on its
# first match, the upstream dies of SIGPIPE (141), and pipefail reports the whole
# pipeline failed — so an arm goes red on correct code, non-deterministically.
# Every arm here feeds grep a here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

TERM_RS=panel/agent/src/routes/terminal.rs
BACKUPS=panel/agent/src/services/backups.rs
MAIN_RS=panel/agent/src/main.rs
SEC_RS=panel/agent/src/routes/security.rs
UTILS=panel/agent/src/routes/server_utils.rs
HEALER=panel/backend/src/services/auto_healer.rs
SETUP=scripts/setup.sh
UPDATE=scripts/update.sh

for f in "$TERM_RS" "$BACKUPS" "$MAIN_RS" "$SEC_RS" "$UTILS" "$HEALER" "$SETUP" "$UPDATE"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT. Copied from the FIXED stripper (lesson #136): a bare
# `s{/\*.*?\*/}{}gs` deleted real code whenever a string literal contained `/*`,
# and an ABSENCE arm then passed on code the stripper had merely removed.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }
has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }
flat()  { tr '\n' ' ' <<< "$1" | tr -s ' '; }
# One function's body, bounded by its OWN closing brace in column 0.
fn_body() { awk -v pat="$1" '$0 ~ pat {f=1} f {print} f && /^\}/ {exit}' <<< "$2"; }

TERM_S=$(subj "$TERM_RS" || true)
BACKUPS_S=$(subj "$BACKUPS" || true)
UTILS_S=$(subj "$UTILS" || true)
HEALER_S=$(subj "$HEALER" || true)

echo "§A  a backup of the panel database is root-only"

# A1 — the retrofit exists and runs at startup. Fixing only the writers would
# leave every install that ALREADY holds a backup exposed for ever, which is the
# s280 install-template delivery shape.
if [ -z "$BACKUPS_S" ]; then
  bad "A1 SKIPPED: $BACKUPS did not yield source"
elif has "$BACKUPS_S" 'pub fn secure_backup_tree' && has "$(subj "$MAIN_RS")" 'secure_backup_tree\(\)'; then
  ok "A1 the backup tree is re-secured at every agent start, so existing installs are repaired"
else
  bad "A1 nothing repairs a backup tree that is already world-readable — new writes alone do not reach an existing install"
fi

# A2 — the modes it enforces. 0700 on the directory is the load-bearing half: at
# that mode nothing under the tree is reachable by a non-root uid whatever the
# individual files say.
if [ -z "$BACKUPS_S" ]; then
  bad "A2 SKIPPED: $BACKUPS did not yield source"
else
  BODY=$(fn_body 'fn secure_tree_at' "$BACKUPS_S")
  BODY_FLAT=$(flat "$BODY")
  if [ -z "$BODY" ]; then
    bad "A2 SKIPPED: could not bound secure_tree_at"
  elif has "$BODY_FLAT" '0o700' && has "$BODY_FLAT" '0o600'; then
    ok "A2 it enforces 0700 on directories and 0600 on files"
  else
    bad "A2 the enforced modes are not 0700/0600 — a dump readable by www-data is a root credential for the whole fleet"
  fi
fi

# A3 — it must never chmod THROUGH a symlink, or a writer that dropped one in the
# tree could aim a root-owned set_permissions at any path on the box.
if [ -z "$BACKUPS_S" ]; then
  bad "A3 SKIPPED: $BACKUPS did not yield source"
else
  BODY=$(fn_body 'fn secure_tree_at' "$BACKUPS_S")
  N=$(count "$BODY" 'is_symlink\(\)')
  if [ "$N" -ge 2 ]; then
    ok "A3 the walk and the chmod both refuse a symlink ($N guards)"
  else
    bad "A3 only $N symlink guard(s) in secure_tree_at — a planted link would redirect a root chmod out of the tree"
  fi
fi

# A4 — EVERY writer, not just the one that was found first. Three produce these
# files: this agent, the panel's auto-healer, and the root crontab line setup.sh
# installs. A fix to one instance of a pattern owes a grep for the pattern.
# ⚠ A4c/A4d ARE SCOPED TO THE BACKUP BLOCK, NOT TO THE FILE. The first draft
# grepped each script file-wide for `umask 077` — and setup.sh ALREADY contained
# three, all guarding `mktemp` calls, so A4c printed GREEN at v2.87.0 over a
# backup cron that wrote 0644. It was a false green hiding inside a batch of
# nineteen reds, where attention goes to the reds (s302). The window below is
# anchored on the block's own BACKUP_DIR assignment, and an empty extraction
# FAILS rather than passes.
# ⚠ TWO correct spellings, and which one is correct depends on the file.
#
# In setup.sh the umask sits inside the `cat > "$BACKUP_SCRIPT" <<'BKEOF'`
# heredoc — a standalone child script, where a bare top-level `umask 077` is
# right and governs only that script.
#
# In update.sh a BARE umask is the REGRESSION, not the fix: v2.88.0 set it at
# top-level scope in an 1100-line installer, so it governed every later `mkdir`
# and redirect and would have created /opt/dockpanel/frontend and the ACME
# challenge directory 0700. s331 scoped it into the subshell that performs the
# dump redirect, which moved this arm's subject (lesson #150 — let the arm
# follow the code). Both forms are accepted here because both protect the dump;
# that a bare one in update.sh is separately FORBIDDEN is pinned by
# upgrade-reach-pin-e2e.sh §A, which is the arm that owns that distinction.
# ⚠ The window is bounded by the OPERATION, not by a line count. It used to be
# `grep -B 6 -A 12` around the BACKUP_DIR assignment, and s331 pushed update.sh's
# dump past +12 simply by documenting the fix above it — so the arm reported the
# dump unprotected while it was protected on the very next line. A fixed -A n
# window is not a block (lesson #172); it silently stops measuring its subject
# the moment anyone writes a comment.
umask_near_backup_block() {
  local f="$1" win
  # From the backup block's own start to the dump it performs, inclusive.
  # Backward context stays a small fixed count — in setup.sh's generated script
  # the umask is the line immediately BEFORE the assignment, and that adjacency
  # is structural. Only the FORWARD span is operation-bounded, because that is
  # the direction commentary grows in.
  win=$( { grep -B 8 -- 'BACKUP_DIR="/var/backups/dockpanel/db"' "$f" 2>/dev/null || true
           awk '/BACKUP_DIR="\/var\/backups\/dockpanel\/db"/{inside=1}
                inside{print}
                inside && /pg_dump/{exit}' "$f"; } )
  [ -n "$win" ] || return 2
  grep -qE 'pg_dump' <<< "$win" || return 2
  grep -qE '^\s*umask 077\s*$|umask 077; docker exec' <<< "$win"
}
A4=0
has "$(subj "$SEC_RS")" 'mode\(0o600\)' || { bad "A4a the agent's own /security/db-backup still creates the dump with the default mode"; A4=1; }
has "$(flat "$HEALER_S")" 'umask 077; docker exec dockpanel-postgres pg_dump' || { bad "A4b the panel's auto-healer dump still lands 0644 (the > redirect creates 0666 \& ~umask)"; A4=1; }
umask_near_backup_block "$SETUP"; case $? in
  0) ;;
  2) bad "A4c could not find the backup block in $SETUP — the reader is broken, not the tree"; A4=1 ;;
  *) bad "A4c setup.sh's installed backup cron still writes a world-readable dump"; A4=1 ;;
esac
umask_near_backup_block "$UPDATE"; case $? in
  0) ;;
  2) bad "A4d could not find the backup block in $UPDATE — the reader is broken, not the tree"; A4=1 ;;
  *) bad "A4d update.sh's pre-upgrade dump still lands world-readable"; A4=1 ;;
esac
[ "$A4" -eq 0 ] && ok "A4 all four writers (agent, auto-healer, setup.sh cron, update.sh) create the dump root-only"

echo "§B  input is judged the same whatever frame shape carried it"

# B1 — THE ARM THAT MATTERS. There is ONE reconstruct-and-classify path and BOTH
# input arms reach it. A unit test cannot assert this: it calls observe_input
# directly and would stay green with an arm reverted to its own inline loop.
if [ -z "$TERM_S" ]; then
  bad "B1 SKIPPED: $TERM_RS did not yield source"
else
  N=$(count "$TERM_S" '= observe_input\(&?(data|text),')
  if [ "$N" -eq 2 ]; then
    ok "B1 both the JSON-frame arm and the raw-text arm go through the one observe_input path"
  else
    bad "B1 $N of 2 input arms call observe_input — a frame shape is being classified differently again, which is the v2.87.0 defect"
  fi
fi

# B2 — the SUSPICIOUS half is NOT gated on the terminal kind. A root shell is
# where an observation is worth the most, and it is the shell an intruder holding
# a stolen admin session opens (see TERMINAL_PIDS' own doc).
if [ -z "$TERM_S" ]; then
  bad "B2 SKIPPED: $TERM_RS did not yield source"
else
  OBS=$(fn_body 'fn observe_input' "$TERM_S")
  OBS_FLAT=$(flat "$OBS")
  if [ -z "$OBS" ]; then
    bad "B2 SKIPPED: could not bound observe_input"
  elif has "$OBS_FLAT" 'if command_filter::is_suspicious_command\(&line\)' \
    && ! has "$OBS_FLAT" 'is_site_terminal && command_filter::is_suspicious_command'; then
    ok "B2 the suspicious check runs on a server shell too, not only on a site shell"
  else
    bad "B2 the suspicious check is gated on the terminal kind — a root shell would emit no intrusion signal"
  fi
fi

# B3 — and the BLOCK half is still site-only, in the same condition. Widening it
# would take `cd ..` from an administrator's server shell, which is a real
# regression in the opposite direction (pinned from the other side in
# client-role-and-server-ownership-pin-e2e.sh §D).
if [ -z "$TERM_S" ]; then
  bad "B3 SKIPPED: $TERM_RS did not yield source"
elif has "$TERM_S" 'is_site_terminal && !command_filter::is_safe_terminal_command'; then
  ok "B3 the block half is still site-only, gated in the same condition as the call"
else
  bad "B3 the block gate and the block call have drifted apart"
fi

echo "§C  a refusal is reported, and does NOT arm the auto-lockdown"

# C1 — a refused command reaches the panel's intrusion channel at all. It used to
# `break` before the classifier and produce one tracing::warn on a target with no
# consumer outside the agent's own journal.
if [ -z "$TERM_S" ]; then
  bad "C1 SKIPPED: $TERM_RS did not yield source"
elif has "$TERM_S" 'write_suspicious_event\("terminal\.blocked_command"'; then
  ok "C1 a refused command is written to the panel's intrusion channel, not only to the journal"
else
  bad "C1 a REFUSED command is still reported to nobody — the strongest observation the product can make is silent"
fi

# C2 — and it is a DISTINCT event type, because the ingest must be able to tell
# the two apart. Keyed on the property (two different type strings reach the
# writer), not on one literal.
if [ -z "$TERM_S" ]; then
  bad "C2 SKIPPED: $TERM_RS did not yield source"
else
  N=$(count "$TERM_S" 'write_suspicious_event\("terminal\.(blocked|suspicious)_command"')
  if [ "$N" -eq 2 ]; then
    ok "C2 a refusal and an allowed-but-suspicious line are distinguishable event types"
  else
    bad "C2 found $N of 2 distinct terminal event types — the ingest cannot separate a refusal from a suspicious run"
  fi
fi

# C3 — THE ARM AGAINST MY OWN REMEDY. `record_suspicious_event_at` counts EVERY
# row in suspicious_events inside the window toward `security_lockdown_threshold`
# (5 in 10 minutes) and a trip blocks every non-admin for 24h. TERMINAL_BLOCKED_
# PATTERNS contains a bare ".." SUBSTRING — its own comment admits `echo "done..."`
# is refused — so a tenant produces refusals BY ACCIDENT. Counting them would turn
# a deliberately blunt blocklist into a self-inflicted outage: a remedy that ARMS
# an escalation, which is this project's second most common failure.
if [ -z "$HEALER_S" ]; then
  bad "C3 SKIPPED: $HEALER did not yield source"
else
  ING=$(fn_body 'async fn security_ingest_suspicious_events' "$HEALER_S")
  ING_FLAT=$(flat "$ING")
  if [ -z "$ING" ]; then
    bad "C3 SKIPPED: could not bound security_ingest_suspicious_events"
  elif has "$ING_FLAT" 'counts_toward_lockdown = event_type != "terminal\.blocked_command"' \
    && has "$ING_FLAT" 'if counts_toward_lockdown'; then
    ok "C3 a refusal is audited but does NOT count toward auto-lockdown — a blunt blocklist cannot lock the panel out"
  else
    bad "C3 refusals feed the auto-lockdown counter — 5 accidental '..' refusals in 10 minutes would block every non-admin for 24h"
  fi
fi

# C4 — but it is still AUDITED, i.e. the operator can see it. audit_log writes
# security_audit_log, which is what SecurityHardening.tsx renders. A fix that made
# refusals invisible in a different way would be no fix at all.
if [ -z "$HEALER_S" ]; then
  bad "C4 SKIPPED: $HEALER did not yield source"
else
  ING=$(fn_body 'async fn security_ingest_suspicious_events' "$HEALER_S")
  if has "$(flat "$ING")" 'security_hardening::audit_log\('; then
    ok "C4 every ingested event still reaches the audit log the panel renders"
  else
    bad "C4 the ingest no longer audits — a refusal would be silent again, by a different route"
  fi
fi

echo "§D  the root shell's environment and privilege drop survive to execv"

# D1 — no `putenv` in the fork child. It STORES THE POINTER; every variable was a
# CString bound inside the block that set it, so Rust freed it at that block's
# closing brace while environ kept pointing at the freed chunk — and execv runs
# after those braces. Measured: 17 of 17 asciicast recordings on the demo box open
# `root@host:/root#` where a live HOME=/root renders `root@host:~#`. HOME was dead
# in every root terminal this panel has ever served.
if [ -z "$TERM_S" ]; then
  bad "D1 SKIPPED: $TERM_RS did not yield source"
else
  N_PUT=$(count "$TERM_S" 'libc::putenv')
  N_SET=$(count "$TERM_S" 'libc::setenv')
  if [ "$N_PUT" -eq 0 ] && [ "$N_SET" -ge 4 ]; then
    ok "D1 the child sets its environment with setenv (which COPIES); no putenv survives ($N_SET setenv)"
  else
    bad "D1 $N_PUT putenv call(s) remain — putenv stores the pointer, and every one of these CStrings is freed before execv"
  fi
fi

# D2 — the privilege drop is CHECKED. A failed setuid (EAGAIN at RLIMIT_NPROC is
# the reachable one, since www-data also runs every PHP-FPM worker) used to fall
# through to execv and hand the SITE'S OWNER a root shell in their own webroot.
if [ -z "$TERM_S" ]; then
  bad "D2 SKIPPED: $TERM_RS did not yield source"
else
  BODY=$(fn_body 'fn open_pty_shell' "$TERM_S")
  BODY_FLAT=$(flat "$BODY")
  if [ -z "$BODY" ]; then
    bad "D2 SKIPPED: could not bound open_pty_shell"
  # ⚠ MATCHED AS ONE CONSTRUCT, not as three tokens. The first draft asserted the
  # three comparisons were PRESENT, and a mutation survived it: prefixing the
  # condition with `false &&` leaves every one of those substrings intact while
  # making the guard unreachable, so the arm printed green over the exact defect
  # it exists to catch (#321 — a token that a rewrite keeps is a spelling test).
  # The `if` must OPEN on setgid and the body must exit.
  elif has "$BODY_FLAT" 'if libc::setgid\(gid\) != 0 \|\| libc::initgroups\(username, gid\) != 0 \|\| libc::setuid\(uid\) != 0 \{[^}]*_exit\(1\)'; then
    ok "D2 all three privilege-drop syscalls gate an _exit — a failed drop cannot reach execv"
  else
    bad "D2 a privilege-drop syscall's result is discarded or no longer gates the exit — a failed drop execs a ROOT shell for the site's owner"
  fi
fi

# D3 — and the result is re-read from the kernel, which is the half that cannot be
# fooled by a return code nobody read.
if [ -z "$TERM_S" ]; then
  bad "D3 SKIPPED: $TERM_RS did not yield source"
else
  BODY_FLAT=$(flat "$(fn_body 'fn open_pty_shell' "$TERM_S")")
  if has "$BODY_FLAT" 'libc::getuid\(\) != uid \|\| libc::geteuid\(\) != uid'; then
    ok "D3 the child asks the kernel who it actually is before exec'ing a shell"
  else
    bad "D3 nothing confirms the drop actually happened"
  fi
fi

echo "§E  \"no SSH keys\" means the agent looked"

# E1 — the read no longer swallows its error. This is the false all-clear: the
# panel's only view of who can SSH in as root answered 200 with an empty list on
# a box with three live keys, on every install, because ProtectHome=yes hides
# /root from the agent and the error became an empty string.
if [ -z "$UTILS_S" ]; then
  bad "E1 SKIPPED: $UTILS did not yield source"
else
  BODY=$(fn_body 'async fn list_ssh_keys' "$UTILS_S")
  BODY_FLAT=$(flat "$BODY")
  # ⚠ KEYED ON THE PROPERTY, NOT ON `unwrap_or_default`. The first draft of this
  # arm forbade that token inside the function and went RED on correct code:
  # `remove_ssh_key` carries one for an unrelated base64 decode, and
  # `list_ssh_keys` legitimately defaults the OPTION (absent file = no keys) once
  # the `?` has already propagated the READ error. The property is that the read
  # goes through the checked helper and its failure propagates — an absent file
  # and an unreadable one must be different answers.
  # Scoped to the three SSH handlers, and keyed on the OPERATION rather than on a
  # spelling. Two earlier drafts were wrong in opposite directions: matching a
  # bare `read_to_string(path)` went red on `get_whitelist` (a different handler,
  # a different file), and matching the literal path went VACUOUS at v2.87.0,
  # where the old code bound `let path = "/root/.ssh/authorized_keys"` first and
  # so spelled neither. Counting in-process fs calls inside the handlers is what
  # actually distinguishes the two trees: 6 at v2.87.0, 0 here.
  SSH_FNS=$(printf '%s\n%s\n%s\n' \
    "$(fn_body 'async fn list_ssh_keys' "$UTILS_S")" \
    "$(fn_body 'async fn add_ssh_key' "$UTILS_S")" \
    "$(fn_body 'async fn remove_ssh_key' "$UTILS_S")")
  N_RAW=$(count "$SSH_FNS" 'tokio::fs::')
  if [ -z "$BODY" ]; then
    bad "E1 SKIPPED: could not bound list_ssh_keys"
  elif has "$BODY_FLAT" 'read_authorized_keys\(\) \.await \.map_err\(' && [ "$N_RAW" -eq 0 ]; then
    ok "E1 an unreadable authorized_keys propagates as an error; only an ABSENT one is an empty list"
  else
    bad "E1 list_ssh_keys still turns a failed read into 'root has no keys' ($N_RAW raw in-process reads) — a false all-clear on root access"
  fi
fi

# E2 — and the read actually escapes the sandbox, or E1 only changes 200-with-a-lie
# into a permanent 500. ⚠ ReadWritePaths= and BindPaths= do NOT override
# ProtectHome=yes — both were measured in a transient unit and both still hide the
# file — so an unsandboxed command is the fix that works without loosening the
# sandbox `agent_unit.rs` exists to protect.
if [ -z "$UTILS_S" ]; then
  bad "E2 SKIPPED: $UTILS did not yield source"
else
  RD=$(fn_body 'async fn read_authorized_keys' "$UTILS_S")
  WR=$(fn_body 'async fn write_authorized_keys' "$UTILS_S")
  if has "$(flat "$RD")" 'safe_command_unsandboxed' && has "$(flat "$WR")" 'safe_command_unsandboxed'; then
    ok "E2 both the read and the write run outside the agent's ProtectHome sandbox"
  else
    bad "E2 an in-process fs call still touches /root/.ssh — ProtectHome=yes makes it answer ENOENT for a file that exists"
  fi
fi

# E3 — remove_ssh_key must NOT treat an unreadable file as empty. THE ORDER OF THE
# TWO FIXES MATTERS: on a failed read the old code filtered every line out of an
# empty string and wrote back "\n", i.e. it DELETED EVERY ROOT SSH KEY. Today that
# is inert only because the write fails under the same sandbox, so repairing the
# sandbox alone would have converted a dead feature into a destructive one.
if [ -z "$UTILS_S" ]; then
  bad "E3 SKIPPED: $UTILS did not yield source"
else
  BODY=$(fn_body 'async fn remove_ssh_key' "$UTILS_S")
  BODY_FLAT=$(flat "$BODY")
  # Same re-keying as E1, and here the property is sharper still: this function
  # REWRITES the file, so an empty default is not merely a wrong answer, it is a
  # destructive one. Neither the read error nor the absent case may fall through
  # to a write — hence `map_err(...)?` AND `ok_or_else(...)?`.
  if [ -z "$BODY" ]; then
    bad "E3 SKIPPED: could not bound remove_ssh_key"
  elif has "$BODY_FLAT" 'read_authorized_keys\(\) \.await \.map_err\(' \
    && has "$BODY_FLAT" '\.ok_or_else\('; then
    ok "E3 a remove on an unreadable or absent key file refuses instead of truncating it to nothing"
  else
    bad "E3 remove_ssh_key can still reach its write with an empty buffer — that WIPES every root SSH key on the box"
  fi
fi

# E4 — the written file is 0600 and owned by root, because sshd ignores an
# authorized_keys that is group- or world-writable and would silently stop
# honouring every key.
if [ -z "$UTILS_S" ]; then
  bad "E4 SKIPPED: $UTILS did not yield source"
else
  WR_FLAT=$(flat "$(fn_body 'async fn write_authorized_keys' "$UTILS_S")")
  if has "$WR_FLAT" '"-m", "600"' && has "$WR_FLAT" 'mode\(0o600\)'; then
    ok "E4 the installed key file is 0600, and so is the staging copy it is built in"
  else
    bad "E4 the key file or its staging copy is not 0600 — sshd ignores a loose authorized_keys"
  fi
fi

echo
echo "PASS $PASS / FAIL $FAIL"
[ "$FAIL" -eq 0 ]
