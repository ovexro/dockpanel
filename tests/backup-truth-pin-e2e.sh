#!/usr/bin/env bash
# backup-truth-pin-e2e.sh — s292 / v2.50.0
#
# Pins the backup defects fixed this release. Pure source analysis: no box, no
# network, no build.
#
# The arms are written against the CAPABILITY a regression must use, not against
# the spelling the code happens to have today (lesson #122: the previous
# provision-log suite passed 30/30 while five reintroductions walked past it).
#
# NO PIPES INTO `grep -q`. Under `set -o pipefail`, grep -q closes the pipe on
# its first match, the upstream process dies of SIGPIPE (141), and pipefail
# reports the PIPELINE as failed — so an arm goes red on correct code, and does
# it non-deterministically. backup-lands-pin-e2e.sh's first draft shipped with
# exactly this bug. Every arm here feeds grep a here-string instead.
#
# Comment handling: every arm reads COMMENT-STRIPPED source (line AND block), so
# a comment can neither satisfy nor break an arm — unlike
# site-backup-databases-pin-e2e.sh, which strips nothing.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

ORCH=panel/backend/src/routes/backup_orchestrator.rs
EXEC=panel/backend/src/services/backup_policy_executor.rs
SCHED=panel/backend/src/services/backup_scheduler.rs
DEST=panel/backend/src/routes/backup_destinations.rs
BACKUPS=panel/backend/src/routes/backups.rs
PREREQ=panel/backend/src/services/prerequisites/backups.rs
UI=panel/frontend/src/pages/BackupOrchestrator.tsx
GUIDE=docs/guides/backup-orchestrator.md
CRYPTO=panel/backend/src/services/secrets_crypto.rs
KEYVER_MIG=panel/backend/migrations/20260831000000_backup_encryption_key_version.sql

for f in "$ORCH" "$EXEC" "$SCHED" "$DEST" "$BACKUPS" "$PREREQ" "$UI" "$GUIDE" "$CRYPTO" "$KEYVER_MIG"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT.
#
# The obvious `s{/\*.*?\*/}{}gs` is wrong and shipped in four suites before this
# one: `/*` occurs INSIDE string literals (a Dockerfile's
# `COPY --from=builder /app/target/release/*`, a glob, a regex), so it opened a
# "block comment" that ran to the next `*/` and deleted real code —
# git_build.rs 1214 -> 729 lines, agent/routes/nginx.rs 2263 -> 2145. A
# truncated subject makes an ABSENCE arm pass on code that was merely deleted by
# the stripper, which is the worst way for a pin to be wrong.
#
# So a block comment is only recognised where one is actually written: opening at
# the start of a line, closing at the end of one. A `/*` in the middle of a line
# is data.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

# An arm whose subject could not be extracted must SKIP, not print a confident
# green next to a red about the same subject (lesson #122b).
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

# has/hasnt/count all take the already-extracted source as $1.
has()   { grep -qE -- "$2" <<< "$1"; }
count() { grep -cE -- "$2" <<< "$1" || true; }

echo "== §1  the SLA queries derive a site's server instead of inventing a column =="
if S=$(subj "$ORCH"); then
  # `backups` has no server_id column and never has. A union branch that labels
  # itself 'site' AND selects a server must therefore reach it through `sites`.
  # Checked as a two-line window rather than a literal, so a differently-spelled
  # reintroduction fails too.
  SITE_SRV=$(count "$S" "server_id, '(site)'::text")
  WINDOW=$(grep -A1 -E "server_id, '(site)'::text" <<< "$S" || true)
  SITE_SRV_JOINED=$(count "$WINDOW" "FROM backups b JOIN sites s")
  if [ "$SITE_SRV" -ge 2 ] && [ "$SITE_SRV" = "$SITE_SRV_JOINED" ]; then
    ok "every site branch that selects a server joins sites ($SITE_SRV/$SITE_SRV_JOINED)"
  else
    bad "a site branch selects a server without joining sites ($SITE_SRV branches, $SITE_SRV_JOINED joined)"
  fi

  # The pre-fix literal, kept as a cheap second opinion on the window arm.
  if has "$S" "SELECT id, created_at, server_id, 'site'"; then
    bad "the original defect literal is back in an SLA CTE"
  else
    ok "no SLA CTE selects server_id straight off backups"
  fi

  # A failed measurement that renders as an empty state is the defect this
  # release removes; the flag is what lets the UI tell them apart.
  if has "$S" "sla_unavailable"; then
    ok "health reports whether the SLA could be measured at all"
  else
    bad "sla_unavailable is gone — a failed SLA query is indistinguishable from no backups again"
  fi
else
  echo "  - skipped §1 (subject unreadable)"
fi
if S=$(subj "$UI"); then
  if has "$S" "sla_unavailable"; then
    ok "the card distinguishes 'could not measure' from 'nothing to measure'"
  else
    bad "the card renders a failed measurement as its empty state again"
  fi
else
  echo "  - skipped §1 UI (subject unreadable)"
fi

echo "== §2  a site that has NEVER been backed up is representable =="
if S=$(subj "$ORCH"); then
  # The HAVING clause deliberately admits sites with no backup at all, and
  # ORDER BY NULLS FIRST puts them on row 1 — so a non-Option row type fails the
  # whole fetch, not just that row.
  if has "$S" "last_backup: Option<chrono::DateTime"; then
    ok "StaleBackup.last_backup is Option"
  else
    bad "StaleBackup.last_backup is not Option — a never-backed-up site cannot decode"
  fi
  if has "$S" "days_since: Option<i64>"; then
    ok "StaleBackup.days_since is Option"
  else
    bad "StaleBackup.days_since is not Option"
  fi
  if has "$S" "Vec<\(String, Option<chrono::DateTime"; then
    ok "the stale-site query decodes into an Option"
  else
    bad "the stale-site query row type no longer admits NULL"
  fi
fi
if S=$(subj "$UI"); then
  if has "$S" "never backed up"; then
    ok "the UI names the never-backed-up case instead of printing an arithmetic artefact"
  else
    bad "the UI lost the never-backed-up rendering"
  fi
fi

echo "== §3  no backup is reported as a success the panel cannot list =="
# Dropping a Result requires spelling `let _ =` in front of the statement.
for f in "$EXEC" "$SCHED" "$BACKUPS"; do
  if S=$(subj "$f"); then
    W=$(grep -A1 -E 'let _ = sqlx::query\(' <<< "$S" || true)
    N=$(count "$W" '"INSERT INTO (backups|database_backups|volume_backups)')
    if [ "$N" = "0" ]; then
      ok "$(basename "$f"): no INSERT into a backup table discards its Result"
    else
      bad "$(basename "$f"): $N backup INSERT(s) still discard their Result"
    fi
  fi
done
if S=$(subj "$EXEC"); then
  # `failures` is what downgrades the status and lights the incident + alert.
  N=$(count "$S" "could not be recorded")
  if [ "$N" -ge 3 ]; then
    ok "all three policy kinds report an unrecorded backup ($N)"
  else
    bad "expected 3 unrecorded-backup reports in the executor, found $N"
  fi
fi
if S=$(subj "$SCHED"); then
  if has "$S" "could not be recorded"; then
    ok "the scheduler fails the run when the row does not land"
  else
    bad "the scheduler no longer fails the run when the row does not land"
  fi
fi
if S=$(subj "$BACKUPS"); then
  if has "$S" "Backup not recorded"; then
    ok "the manual path's live log reports an unrecorded backup"
  else
    bad "the manual path emits a green complete over an unrecorded backup again"
  fi
  # That branch must NOT return early: the tail of the spawned task removes the
  # provisioning log from the shared map, so returning leaks it and its owner.
  #
  # Guarded on a non-empty window. Without the guard this arm passes vacuously
  # whenever the branch is absent — printing a confident green beside the red
  # about the very same subject (lesson #122b).
  W=$(grep -A3 "Backup not recorded" <<< "$S" || true)
  if [ -z "$W" ]; then
    bad "cannot check the unrecorded-backup branch: it is not there"
  elif [ "$(count "$W" '^\s*return;')" = "0" ]; then
    ok "the unrecorded-backup branch falls through to the log cleanup"
  else
    bad "the unrecorded-backup branch returns early and leaks the provisioning log"
  fi
fi

echo "== §4  a stored destination credential is never destroyed nor echoed =="
if S=$(subj "$DEST"); then
  if has "$S" "fn reject_empty_secrets"; then
    ok "an empty secret is refused rather than written over the stored one"
  else
    bad "reject_empty_secrets is gone — an empty secret can destroy the credential again"
  fi
  M=$(count "$S" "mask_config_secrets\(&mut dest\)")
  if [ "$M" -ge 2 ]; then
    ok "create and update both mask before returning ($M)"
  else
    bad "expected >=2 masked returning paths, found $M"
  fi
  # test_connection must NOT mask — it feeds the agent the real credential.
  # Only meaningful once a masker exists; asserting its absence in a tree that
  # has no masker at all is a green about nothing.
  W=$(grep -A25 "pub async fn test_connection" <<< "$S" || true)
  if [ "$M" -lt 2 ] || [ -z "$W" ]; then
    bad "cannot check test_connection: no masker to look for"
  elif [ "$(count "$W" "mask_config_secrets")" = "0" ]; then
    ok "test_connection is deliberately unmasked"
  else
    bad "test_connection masks its row — the agent would authenticate with the sentinel"
  fi
  if has "$S" "fn carry_unmentioned_keys"; then
    ok "a stored key the form does not send survives an edit"
  else
    bad "carry_unmentioned_keys is gone — an unrelated edit erases key_path again"
  fi
fi

echo "== §5  a policy can be edited, and an omitted field is not a reset =="
if S=$(subj "$ORCH"); then
  if has "$S" "struct UpdatePolicyRequest"; then
    ok "update has its own request type"
  else
    bad "update shares CreatePolicyRequest again — absent and default collapse"
  fi
  # The three columns the UPDATE used to assign unconditionally.
  for col in server_id destination_id retention_count; do
    if has "$S" "req\.$col\.unwrap_or\(cur_"; then
      ok "omitted keeps its current value: $col"
    else
      bad "reset-on-omission is back for $col"
    fi
  done
  # Explicit null must still clear a destination, or local-only is unreachable.
  if has "$S" "de_double_option"; then
    ok "explicit null is distinguishable from omitted"
  else
    bad "explicit null and omitted are the same value again"
  fi
fi
if S=$(subj "$UI"); then
  if has "$S" 'api\.put\(.\/backup-orchestrator\/policies\/'; then
    ok "the policy edit form calls PUT"
  else
    bad "nothing in the browser calls the policy PUT — it is create-once again"
  fi
  if has "$S" "policies/protect-all"; then
    ok "Protect Everything has a caller"
  else
    bad "Protect Everything is dead again while the guide still says to click it"
  fi
fi
if S=$(subj "$PREREQ"); then
  # The advice must point at the screen that can carry it out.
  W=$(grep -A4 '"Choose a destination"' <<< "$S" || true)
  if [ "$(count "$W" "tab=policies")" != "0" ]; then
    ok "'Choose a destination' links to where a policy is edited"
  else
    bad "'Choose a destination' points away from the only control that can do it"
  fi
fi

echo "== §6  Encrypt promises only what the agent can deliver =="
if S=$(subj "$EXEC"); then
  if has "$S" "encryption applies to database dumps only"; then
    ok "the executor says so when a policy encrypts and also covers sites/volumes"
  else
    bad "the executor is silent about unencrypted site/volume archives again"
  fi
fi
if S=$(subj "$UI"); then
  if has "$S" "Encrypt DB dumps"; then
    ok "the checkbox names its scope"
  else
    bad "the checkbox promises unqualified encryption again"
  fi
fi
# The guide is prose, so it is read raw on purpose.
if grep -q "database dumps only" "$GUIDE"; then
  ok "the guide scopes encryption to database dumps"
else
  bad "the guide promises unqualified AES-256 again"
fi
# If a future change gives site/volume archives a real encryption path, THIS is
# the arm that should go red first, so the copy above gets revisited with it.
ENC_FILES=$(grep -rl "encrypt_file" panel/agent/src/ 2>/dev/null | wc -l)
if [ "$ENC_FILES" -le 2 ]; then
  ok "agent-side encryption is still database-only ($ENC_FILES file(s))"
else
  bad "agent encryption spread to $ENC_FILES files — re-check the Encrypt copy"
fi

echo "== §7  remote retention that is not enforced is reported =="
for f in "$EXEC" "$SCHED"; do
  if S=$(subj "$f"); then
    # Keyed on DESTRUCTURING, not on one discard spelling. Forbidding
    # `let _ = agent.post("/backups/prune"` was evaded in testing by
    # `let _unused = ….ok(); if false {`, which kept the message-handling code
    # in the file — and therefore kept the sibling arm green — while it could
    # never run. To read the response you have to open a block on it; no form of
    # discard does.
    W=$(grep -A12 '"/backups/prune"' <<< "$S" || true)
    if [ -z "$W" ]; then
      bad "$(basename "$f"): no prune call found at all"
    elif [ "$(count "$W" '\.await \{')" = "0" ]; then
      bad "$(basename "$f"): the prune response is not destructured — it cannot be read"
    elif [ "$(count "$W" 'let _|\.ok\(\)|if false')" != "0" ]; then
      bad "$(basename "$f"): the prune response is discarded or its handling is dead"
    else
      ok "$(basename "$f"): the prune response is destructured and read"
    fi
    # The message must reach a durable operator surface, not just a journal.
    #
    # ⚠ This used to look for `log_event` inside a fixed `grep -A12` window taken
    # FORWARD from the message, with a second forward window from the prune call
    # as a fallback. But the call WRAPS the message — `log_event(db, level,
    # source, &format!("…"))` — so the token being sought sits ABOVE the token
    # being searched from, and both windows only ever found it by accident of
    # spacing. At s334 a five-line comment added above the call pushed it out of
    # the fallback window and this arm went red on correct code (#333, recurring:
    # a fixed forward window is not a statement, and prose displaces it).
    #
    # Now bounded by the CALL itself: a `log_event(` whose argument list runs to
    # the end of the statement and contains the message. Spacing and comments
    # cannot move that relationship. Mutation-verified: severing the message from
    # log_event turns this red while its sibling file stays green.
    if [ "$(count "$S" 'emote retention was not enforced')" = "0" ]; then
      bad "$(basename "$f"): nothing records that retention was a no-op"
    elif perl -0777 -ne 'exit(!(/log_event\s*\([^;]*?emote retention was not enforced/s))' <<< "$S"; then
      ok "$(basename "$f"): an unenforceable retention reaches system_log"
    else
      bad "$(basename "$f"): the unenforced-retention message never reaches system_log"
    fi
  fi
done

echo "== §8  the chain report states only what the panel actually recorded (s311) =="
#
# `chain_valid` was written TRUE by all seven of its INSERTs — five literal, two
# by column default — UPDATEd nowhere, and read back with `.unwrap_or(true)`.
# `chain-report.typ` then rendered it TWICE as a bold green YES, under a footer
# telling the reader that a break "indicates either a missing intermediate backup
# or a tampered artifact". No verifier existed, and none could: `previous_hash` is
# copied out of the newest row rather than recomputed from the predecessor
# artifact. The document certified tamper-evidence the system had no mechanism to
# evaluate, in a PDF built to be handed to an auditor.
#
# The column and its writers survive (dropping a column is a migration, and this
# release does not need one). What must not come back is a RENDERED or SERVED
# verdict. So these arms are about readers and outputs, never about the INSERTs.
TYP=panel/backend/templates/chain-report.typ
if [ ! -f "$TYP" ]; then
  bad "MISSING SUBJECT FILE: $TYP"
else
  TYP_SRC=$(cat "$TYP")
  # CONTROL FIRST. Both arms below are absence arms, and an absence arm over an
  # unreadable or renamed subject is the most reassuring way for a pin to be
  # wrong (lesson #143). If the template no longer renders the two hash rows,
  # this section is measuring a file that is not the chain report any more.
  if [ "$(count "$TYP_SRC" 'SHA-256')" != "0" ] && [ "$(count "$TYP_SRC" 'Previous hash')" != "0" ]; then
    ok "the chain report still renders the hashes it genuinely recorded (control)"
  else
    bad "$TYP does not render SHA-256/Previous hash — the absence arms below mean nothing"
  fi

  if [ "$(count "$TYP_SRC" 'Chain valid|chain_valid')" = "0" ]; then
    ok "the report renders no chain-validity verdict"
  else
    bad "the report renders a chain-validity verdict again — nothing can compute it"
  fi

  # The footer prose is the half that made it an assertion rather than a field:
  # it told the reader what a break MEANS. A verdict nobody computes must not
  # come back wearing an explanation.
  if [ "$(count "$TYP_SRC" 'tampered artifact|break in the chain')" = "0" ]; then
    ok "the footer no longer explains a verdict the panel cannot reach"
  else
    bad "the footer claims a chain break implies tampering — that claim is unbacked"
  fi
fi

if S=$(subj "$ORCH"); then
  # Every surviving mention must be a WRITE. A reader is what puts the verdict
  # back on a surface — the struct field, the SELECT list, the .unwrap_or(true).
  TOTAL=$(count "$S" 'chain_valid')
  WRITES=$(count "$(grep -E 'INSERT INTO' <<< "$S" || true)" 'chain_valid')
  if [ "$TOTAL" = "$WRITES" ]; then
    ok "chain_valid survives only in INSERTs — no reader, no DTO field ($WRITES)"
  else
    bad "chain_valid has $((TOTAL - WRITES)) non-INSERT occurrence(s) — something reads the verdict again"
  fi

  # The section itself must survive with its two genuinely computed fields, or
  # the arms above would also pass on a report that simply lost the section.
  if has "$S" 'verifications_passed' && has "$S" 'drills_passed'; then
    ok "the chain-integrity section keeps the two counts it can actually derive"
  else
    bad "the chain-integrity section lost its computed fields — this was a deletion too far"
  fi
fi

# The same claim lived in a published guide, and there it was stronger than the
# PDF's: "If an attacker tampers with backup files, the chain breaks and alerts
# fire." Nothing re-computes a hash and no alert exists, so removing the verdict
# from the report while leaving that sentence up would have moved the lie rather
# than retiring it. Prose, so it is read raw on purpose.
HARDEN=docs/guides/security-hardening.md
if [ ! -f "$HARDEN" ]; then
  bad "MISSING SUBJECT FILE: $HARDEN"
elif ! grep -q 'SHA-256' "$HARDEN"; then
  bad "$HARDEN no longer describes backup hashing — this arm is measuring nothing"
elif grep -qi 'the chain breaks and alerts fire' "$HARDEN"; then
  bad "the hardening guide promises tamper alerts that no code can fire"
else
  ok "the hardening guide does not promise tamper detection the panel cannot do"
fi

echo "== §9  no backup/site query names a column the schema does not have (s311) =="
#
# `sites.rs` health_summary selected `last_response_ms` from `monitors`, whose
# column is `last_response_time` — a 42703 at parse-analyze, so an unconditional
# 500 from the day it shipped (e65893c, 2026-03-22) until v2.68.0. The backend
# has ZERO compile-checked queries (`sqlx::query!` count is 0 against 728
# unchecked `query_as(`), so nothing but execution could ever have caught it.
#
# ⚠ This is a REGRESSION pin on the one literal, not the class sweep. The class
# arm — every identifier in a SELECT list exists in the migrations for that table
# — needs either a live database or a migration parser, and neither belongs in a
# pure-source suite. Recorded here so the next person knows what is NOT covered.
BAD_COL=$(grep -rn 'last_response_ms' --include=*.rs panel/backend/src panel/agent/src 2>/dev/null | wc -l)
GOOD_COL=$(grep -rn 'last_response_time' --include=*.rs panel/backend/src panel/agent/src 2>/dev/null | wc -l)
if [ "$GOOD_COL" -eq 0 ]; then
  bad "neither column name appears in the backend — this arm is measuring nothing"
elif [ "$BAD_COL" -eq 0 ]; then
  ok "no query selects last_response_ms ($GOOD_COL use(s) of the real column as control)"
else
  bad "$BAD_COL query/queries name last_response_ms again — monitors has no such column"
fi

echo
echo "== §10  every backup INSERT records the integrity hash (#114) =="
#
# Two of the seven INSERTs — BOTH unattended site paths — omitted sha256_hash and
# previous_hash while every database and volume insert, including the two in the
# same function as one of them, carried both. The agent had computed the hash on
# every one of those runs and the backend read the two fields beside it and
# dropped this one. A user found it by querying his own table; nothing here could
# have, because the only pin near it froze a column LIST instead of asserting
# parity, so it could not see an asymmetry and turned red when the fix arrived.
#
# The arm is a PARITY census, not a list: enumerate every insert into the three
# backup tables and require each to name both columns. `chain_valid` is
# deliberately NOT required — nothing computes it, five inserts write it as a
# literal and two take the column default, and §8 above pins that the report
# renders no verdict from it.
#
# Unit: INSERT STATEMENTS (not lines) — each `INSERT INTO <t> (` and its
# backslash-continued column list joined, via perl. Instrument named per #525.
INS_REPORT=""
INS_TOTAL=0
INS_BAD=0
for f in "$ORCH" "$EXEC" "$SCHED" "$BACKUPS"; do
  while IFS= read -r stmt; do
    [ -n "$stmt" ] || continue
    INS_TOTAL=$((INS_TOTAL+1))
    if ! has "$stmt" 'sha256_hash' || ! has "$stmt" 'previous_hash'; then
      INS_BAD=$((INS_BAD+1))
      INS_REPORT="$INS_REPORT $(basename "$f")"
    fi
  done < <(perl -0777 -ne '
      while (/INSERT\s+INTO\s+(backups|database_backups|volume_backups)\s*\((.*?)\)/gs) {
        my $c = $2; $c =~ s/\s*\\\s*\n\s*/ /g; $c =~ s/\s+/ /g; print "$1 ($c)\n";
      }' "$f")
done
if [ "$INS_TOTAL" -lt 7 ]; then
  bad "§10 found only $INS_TOTAL backup INSERT(s) — expected at least 7, the arm is under-reading its subjects"
elif [ "$INS_BAD" -eq 0 ]; then
  ok "all $INS_TOTAL backup INSERTs record sha256_hash + previous_hash"
else
  bad "$INS_BAD of $INS_TOTAL backup INSERTs omit the integrity hash —$INS_REPORT"
fi

echo "== §11  the chain report's Typst has no markup-mode conditional (#113) =="
#
# `#let kind_label = if …{}` with its `else if` branches on the FOLLOWING lines
# was written in MARKUP mode, where a `#`-expression ends at the first line break
# that leaves it complete. So only the first branch bound, the rest were re-lexed
# as a paragraph and PRINTED above the masthead, the label came out empty for two
# of the three backup kinds, and the drills table discarded every body_excerpt it
# was supposed to show. The suite that renders this document asserts HTTP 200,
# the content type and the %PDF magic — a PDF full of leaked source passes all of
# it, which is why a reporter found this and CI did not.
#
# Instrument: for every line that begins a continuation branch, walk BACKWARD to
# the nearest line whose first non-space character is `#`, and flag it unless
# that opener ends in `{` (a code block, where newlines are trivia).
# ⚠ Do NOT write this with `\b` — GNU awk treats it as a backspace, and the first
# draft of this arm reported zero leaks against the visibly broken file.
leakcheck() {
  perl -0777 -ne '
    my @l = split /\n/, $_; my $bad = 0;
    for my $i (0 .. $#l) {
      next unless $l[$i] =~ /^[ \t]*else[ {\[]/;
      for (my $j = $i - 1; $j >= 0; $j--) {
        next unless $l[$j] =~ /^[ \t]*#/;
        $bad++ unless $l[$j] =~ /\{[ \t]*$/;
        last;
      }
    }
    print "$bad\n";
  ' "$1"
}
TYP_LEAKS=$(leakcheck "$TYP")
# Positive control: the same instrument must still see a planted leak, or a green
# result here means nothing (#480 — an absence arm with no control cannot tell
# failure from absence).
TYP_PLANT=$(mktemp); { cat "$TYP"; printf '#let ov_probe = if 1 == 1 { "a" }\n  else { "b" }\n'; } > "$TYP_PLANT"
TYP_PLANT_LEAKS=$(leakcheck "$TYP_PLANT"); rm -f "$TYP_PLANT"
if [ "$TYP_PLANT_LEAKS" -le "$TYP_LEAKS" ]; then
  bad "§11 the leak detector does not react to a planted markup-mode conditional — the arm is vacuous"
elif [ "$TYP_LEAKS" -eq 0 ]; then
  ok "no markup-mode conditional in the chain report (planted control detected: $TYP_PLANT_LEAKS)"
else
  bad "$TYP_LEAKS markup-mode conditional branch(es) in $TYP — they render as source text in the PDF"
fi

echo "== §12  backup_destinations.encryption_enabled is a documented dead column, not a forgotten one (s425) =="
#
# The struct is the caller-facing shape written by hand — unlike SelectableDestination,
# whose "config absent by construction" comment already explains itself, nothing ever
# explained why encryption_enabled/encryption_key are missing from BackupDestination.
# That silence is exactly what let TWO prior remediation passes on this same migration
# (v2.24.0's encryption_key fix, and a second pass on this table) walk past the sibling
# column: it had zero readers, zero writers, and no comment anywhere.
DEST_RAW=$(cat "$DEST")
STRUCT_BODY=$(sed -n '/^pub struct BackupDestination {/,/^}/p' "$DEST")

if [ -z "$STRUCT_BODY" ]; then
  bad "§12 could not locate 'pub struct BackupDestination' in $DEST — extraction failed, arm skipped"
elif ! grep -qE '^\s*pub id: Uuid,' <<< "$STRUCT_BODY"; then
  bad "§12 struct extraction landed on the wrong block (no 'id' field) — positive control failed"
else
  ok "§12 struct extraction located BackupDestination correctly (positive control: 'id' field present)"
  if grep -qE 'encryption_enabled|encryption_key' <<< "$STRUCT_BODY"; then
    bad "§12 BackupDestination declares an encryption_enabled/encryption_key field — a dead column was reintroduced without removing or updating the tombstone comment above the struct"
  else
    ok "§12 BackupDestination still excludes encryption_enabled/encryption_key (by construction)"
  fi
fi

# The tombstone must live directly above the struct, not merely somewhere in the file —
# an unattached comment gives a future reader no reason to look at it before adding a field.
PRE_STRUCT=$(grep -B12 '^pub struct BackupDestination {' "$DEST")
if grep -q 'encryption_enabled' <<< "$PRE_STRUCT" && grep -q 'encryption_key' <<< "$PRE_STRUCT"; then
  ok "§12 a tombstone comment directly above BackupDestination documents both dead columns"
else
  bad "§12 no comment directly above BackupDestination documents encryption_enabled/encryption_key as dead — a future reader has no way to know they are intentionally unused"
fi

# Regression control: if this ever fires red, the "zero readers/writers" claim the
# tombstone makes is now false and the comment (or the code) needs to change.
ENC_ENABLED_HITS=$(grep -rniE 'encryption_enabled' --include=*.rs --include=*.tsx --include=*.ts panel/ | grep -vF "$DEST" | wc -l)
if [ "$ENC_ENABLED_HITS" -gt 0 ]; then
  bad "§12 encryption_enabled is referenced outside $DEST ($ENC_ENABLED_HITS hit(s)) — the dead-column claim in the tombstone is stale"
else
  ok "§12 encryption_enabled has no reader or writer anywhere else in the tree"
fi

echo "== §13  the backup-encryption passphrase is versioned, not a raw JWT_SECRET substring (s434) =="
#
# v1 (format!("backup-enc-{}", &jwt_secret[..32])) handed anyone who learned a
# backup passphrase 32 bytes of the SAME secret every session token is
# HMAC-signed with. v2 derives it via HKDF instead — a one-way PRF, so the
# passphrase reveals nothing about jwt_secret. Restore must reproduce
# whichever version actually wrote a given file (database_backups.encryption_key_version,
# written at encrypt time), never assume "the current one".

EXEC_SRC=$(code "$EXEC")
CRYPTO_SRC=$(code "$CRYPTO")
ORCH_SRC=$(code "$ORCH")
if [ -z "$EXEC_SRC" ] || [ -z "$CRYPTO_SRC" ] || [ -z "$ORCH_SRC" ]; then
  bad "§13 one of EXEC/CRYPTO/ORCH extracted empty — the extractor is broken, not the tree"
fi

# Every arm below extracts the SPECIFIC function's own body (sed range,
# anchored on its exact signature) and checks what that body actually DOES —
# never "does this name/text appear somewhere in the file", which an
# adversarial review proved is satisfiable by a same-named stub, a decoy
# reference elsewhere in the file, or renaming the parameter before using it.
# A name/signature existing is necessary; it is never sufficient.

V1_FN=$(sed -n '/^fn derive_backup_encryption_key_v1(jwt_secret: &str) -> String {/,/^}/p' <<< "$EXEC_SRC")
if [ -z "$V1_FN" ]; then
  bad "§13a could not extract derive_backup_encryption_key_v1's own body — signature changed or function is gone; a pre-s434 encrypted backup can no longer be restored"
elif has "$V1_FN" 'format!\("backup-enc-\{\}", &jwt_secret\[\.\.32\.min\(jwt_secret\.len\(\)\)\]\)'; then
  ok "§13a derive_backup_encryption_key_v1's own body still performs the exact legacy derivation (needed to decrypt pre-existing backups)"
else
  bad "§13a derive_backup_encryption_key_v1 exists but its BODY no longer performs the legacy derivation — a same-named stub would satisfy a name-only check while making every pre-s434 encrypted backup permanently unrestorable"
fi

V2_FN=$(sed -n '/^pub fn derive_backup_encryption_key_v2(jwt_secret: &str) -> String {/,/^}/p' <<< "$CRYPTO_SRC")
if [ -z "$V2_FN" ]; then
  bad "§13b could not extract derive_backup_encryption_key_v2's own body — signature changed or function is gone"
elif has "$V2_FN" 'hex::encode\(derive_key_with_salt\(jwt_secret, BACKUP_ENCRYPTION_SALT\)\)'; then
  ok "§13b derive_backup_encryption_key_v2's own body derives via HKDF (derive_key_with_salt), not a raw secret slice or the legacy SHA-256-only path"
else
  bad "§13b derive_backup_encryption_key_v2's body no longer calls derive_key_with_salt — it may have regressed to derive_key_legacy (plain SHA-256, the pre-HKDF weakness) or something else entirely, even if that call text still appears elsewhere in the file as a decoy"
fi

# §13c — the REGRESSION this whole section exists to catch: if a future edit
# inlines v1's raw-slice pattern back into the CURRENT-version function
# (derive_backup_encryption_key, not the deliberately-kept-legacy _v1), new
# backups go back to leaking JWT_SECRET bytes.
#
# ⚠ Two prior drafts of this arm were each defeated in review before ship:
# (1) a bare file-wide COUNT of the slice pattern can't tell "correctly
# isolated in v1" from "still in the current function" (pre-fix code also
# shows exactly 1 occurrence); (2) a body-scoped grep for the LITERAL
# identifier `jwt_secret[..` is defeated by aliasing the parameter first
# (`let s = jwt_secret; ...&s[..32]...`) — same leak, different spelling. The
# fix for both: require the CURRENT function's body to be EXACTLY a
# delegation call to derive_backup_encryption_key_for_version — a positive
# match on the one correct shape, not a negative match on the many ways to
# spell the wrong one. A delegation call has no `[` at all; any hand-rolled
# derivation (v1's, or a renamed-variable variant of it) does.
CURRENT_FN=$(sed -n '/^pub fn derive_backup_encryption_key(jwt_secret: &str) -> String {/,/^}/p' <<< "$EXEC_SRC")
if [ -z "$CURRENT_FN" ]; then
  bad "§13c could not extract derive_backup_encryption_key's own body — signature changed, re-read it"
elif ! has "$CURRENT_FN" 'derive_backup_encryption_key_for_version\(CURRENT_BACKUP_KEY_VERSION, jwt_secret\)'; then
  bad "§13c derive_backup_encryption_key's body is no longer a plain delegation to derive_backup_encryption_key_for_version(CURRENT_BACKUP_KEY_VERSION, jwt_secret) — it may be computing a key itself again, under any variable name"
elif has "$CURRENT_FN" '\['; then
  bad "§13c derive_backup_encryption_key's body contains '[' (indexing/slicing) alongside the delegation call — a hand-rolled derivation may have been added back in, even if the delegation call is also still present"
else
  ok "§13c derive_backup_encryption_key's body is exactly a delegation to derive_backup_encryption_key_for_version — no slicing under any spelling"
fi

# §13d — not just that the COLUMN NAME appears in the INSERT text, but that
# the value bound to it is the CURRENT_BACKUP_KEY_VERSION constant specifically
# (not a hardcoded literal that happens to equal today's value, which would
# silently stop tracking future version bumps). Extract the substring between
# the two adjacent, invariant anchors around it in the actual bind() chain.
BIND_GAP=$(sed -n 's/.*\.bind(encrypted)\(.*\)\.bind(policy\.id).*/\1/p' <<< "$EXEC_SRC")
if [ -z "$BIND_GAP" ]; then
  bad "§13d could not isolate the bind() call between .bind(encrypted) and .bind(policy.id) — the INSERT's bind chain shape changed, re-read it"
elif [ "$(tr -d ' ' <<< "$BIND_GAP")" = ".bind(CURRENT_BACKUP_KEY_VERSION)" ]; then
  ok "§13d the INSERT binds encryption_key_version to CURRENT_BACKUP_KEY_VERSION specifically, not a hardcoded literal"
else
  bad "§13d the bind() between encrypted and policy.id is '$BIND_GAP', not .bind(CURRENT_BACKUP_KEY_VERSION) — every new encrypted backup could be recorded under the wrong (e.g. hardcoded, stale) version, making restore derive the wrong passphrase for it"
fi

# §13e — restore must pass the ROW'S OWN recorded version as the FIRST
# ARGUMENT specifically, not merely have both the function name and
# `backup.encryption_key_version` appear SOMEWHERE in the same function (a
# decoy log line referencing the field elsewhere satisfies that weaker
# check while the real call still hardcodes CURRENT_BACKUP_KEY_VERSION).
RESTORE_FN=$(sed -n '/pub async fn restore_db_backup/,/^}/p' <<< "$ORCH_SRC" | tr '\n' ' ' | tr -s ' ')
if [ -z "$RESTORE_FN" ]; then
  bad "§13e could not extract restore_db_backup from $ORCH"
elif has "$RESTORE_FN" 'derive_backup_encryption_key_for_version\( *backup\.encryption_key_version *,'; then
  ok "§13e restore_db_backup passes the row's own encryption_key_version as the version argument, adjacent to the call"
elif has "$RESTORE_FN" 'derive_backup_encryption_key_for_version\('; then
  bad "§13e restore_db_backup calls derive_backup_encryption_key_for_version, but NOT with backup.encryption_key_version as the first argument — it may be hardcoding CURRENT_BACKUP_KEY_VERSION instead, silently deriving the wrong passphrase for any backup written under an older version"
elif has "$RESTORE_FN" 'derive_backup_encryption_key\(&state\.config\.jwt_secret\)'; then
  bad "§13e restore_db_backup calls the version-LESS derive_backup_encryption_key (always current) — a backup written under an older derivation would silently get the wrong passphrase"
else
  bad "§13e restore_db_backup's key derivation matches none of the known shapes — re-read it"
fi

if grep -qE 'ADD COLUMN IF NOT EXISTS encryption_key_version SMALLINT NOT NULL DEFAULT 1' "$KEYVER_MIG"; then
  ok "§13f the migration adds encryption_key_version, defaulting existing rows to 1 (the legacy scheme every pre-existing row actually used)"
else
  bad "§13f the migration file does not add encryption_key_version with the expected default — re-read it"
fi

echo
echo "backup-truth: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
