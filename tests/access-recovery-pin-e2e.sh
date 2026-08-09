#!/usr/bin/env bash
# Regression pins for s332 — a control must not report a state it never established,
# and a lockout must have a way back that does not require the thing that broke.
#
#   R1  THE BOOTSTRAP DOOR LEFT ITS OWN ADMINISTRATOR UNVERIFIABLE. `auth::setup`
#       inserted only (email, password_hash, role), so the first administrator of
#       every install landed `email_verified = FALSE` with `email_token = NULL`.
#       `login` refuses an unverified account whenever `smtp_host` is merely
#       NON-EMPTY, so the gate armed the moment the operator configured mail —
#       against their own account. Both exits needed that mail: the token door
#       matches `WHERE email_token = $1` and this user has no token, so the door
#       does not exist for them, and `reset_password` needs the forgot-password
#       message. Reported as #100 by an operator who reinstalled to get back in.
#       `register` (the sibling door) inserted BOTH columns all along — the
#       ASYMMETRY between two implementations of one step was the bug (#307).
#
#   R2  A SECOND IP WHITELIST WROTE A FILE NOTHING READ. `/etc/dockpanel/
#       panel-whitelist.conf` had exactly two references in the whole repository:
#       its own writer and its own read-back-for-display. No nginx include, no
#       template, nothing compiled in. Its card sat ABOVE the real control on the
#       same settings tab and answered "Whitelist saved (N IPs)".
#
#   R3  AN INTRUSION DETECTOR REPORTED ARMED WHILE UNARMED. Nothing plants the
#       canary files — the writer is an agent route with no caller — and the
#       reader took the same silent `continue` for "never created" as for
#       "cannot read it", so a tripwire with nothing on the wire was
#       indistinguishable from a quiet one. The toggle renders ON by default.
#
# WHY THESE ARMS EXIST IN THIS SHAPE: every arm below was RUN AGAINST v2.89.0 and
# required to be RED there, with its expected polarity written down BEFORE the run
# (#335 — a false green hides inside a batch of reds, because attention goes to the
# reds). The two CONTROL arms are the exceptions and are green at both tags on
# purpose: they assert I removed the INERT allowlist rather than the enforcing one,
# and that the verification gate itself was not weakened to "fix" the lockout.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

BE=panel/backend/src
AG=panel/agent/src
FE=panel/frontend/src

for d in "$BE" "$AG" "$FE" panel/backend/migrations docs; do
  [ -d "$d" ] || { echo "missing dir: $d — refusing to report a clean sweep"; exit 1; }
done

# NOTE ON grep: counts use `grep -c`, never `grep -q` inside a pipeline. Under
# `set -o pipefail` a `grep -q` exits at its first match, the producer dies of
# SIGPIPE 141, and the pipeline reports FAILURE for a SUCCESSFUL match (#334 — this
# turned a KILLED mutation into a reported SURVIVOR one session ago).
#
# NOTE ON WINDOWS: no arm below uses a fixed `grep -A n`. A fixed forward window is
# not a block and rots the moment somebody writes a comment above the subject
# (#333 — my own 14-line explanation silently unhooked an arm at s331). Bodies are
# sliced with perl -0777 to the construct's own terminator.

# Slice a Rust fn body by brace balance, from `fn <name>` to its matching close.
fn_body() {
  perl -0777 -ne '
    if (/\bfn\s+'"$2"'\b/g) {
      my $s = pos($_); my $i = index($_, "{", $s); my $d = 0;
      for (my $j = $i; $j < length($_); $j++) {
        my $c = substr($_, $j, 1);
        $d++ if $c eq "{"; $d-- if $c eq "}";
        if ($d == 0) { print substr($_, $i, $j - $i + 1); last }
      }
    }' "$1"
}

echo "── §A #100: the bootstrap door verifies the administrator it creates ──"

SETUP_BODY=$(fn_body "$BE/routes/auth.rs" setup)
if [ -z "$SETUP_BODY" ]; then
  bad "could not slice auth::setup — the extractor found nothing, so every §A arm below would pass vacuously"
else
  # A1. The INSERT must name email_verified. Scoped to the INSERT statement, not to
  # the function: `setup` is long and mentions users elsewhere.
  ins=$(printf '%s' "$SETUP_BODY" | perl -0777 -ne 'print $1 if /(INSERT INTO users.*?RETURNING \*)/s')
  if [ -n "$ins" ] && [ "$(printf '%s' "$ins" | grep -c 'email_verified')" -gt 0 ]; then
    ok "A1 auth::setup's INSERT names email_verified — the first administrator is not born locked out"
  else
    bad "A1 auth::setup creates the first administrator without email_verified; one SMTP setting locks them out of their own panel (#100)"
  fi

  # A2. And it must be TRUE, not merely mentioned. A mutation that flips the value
  # would otherwise leave A1 green.
  if [ -n "$ins" ] && [ "$(printf '%s' "$ins" | grep -ciE "email_verified[^,]*\)[^)]*'admin',\s*TRUE|'admin',\s*TRUE")" -gt 0 ]; then
    ok "A2 the bootstrap administrator is inserted verified (TRUE), not merely with the column present"
  else
    bad "A2 auth::setup names email_verified but does not insert TRUE — the lockout survives"
  fi
fi

# A3. Existing installs already hold the broken row, so the writer fix does not
# reach them (#332). A migration must release exactly the accounts that can never
# verify: unverified AND holding no token.
# Ask whether ANY migration carries the predicate, rather than picking one and
# testing it: `head -1` over a name-sorted list chose the 2026-03 saas_auth
# migration (it mentions both columns) and reported the release missing while it
# was present — an arm that measured the wrong subject and failed in the
# direction that looks like a real finding.
MIG=$(grep -rl 'email_token IS NULL' panel/backend/migrations --include='*.sql' 2>/dev/null \
      | xargs -r grep -l 'email_verified' 2>/dev/null | head -1)
if [ -n "$MIG" ]; then
  ok "A3 a migration releases accounts that are unverified AND tokenless ($(basename "$MIG"))"
else
  bad "A3 no migration releases already-installed accounts that can never verify — the fix reaches new installs only (#332)"
fi

# A4. An administrator must be able to do it on a running panel. `email_verified`
# had five writers and every one was self-service or machine.
if [ "$(grep -c 'verify-email' "$BE/routes/mod.rs")" -gt 0 ] \
   && [ "$(grep -c 'fn verify_user_email' "$BE/routes/security.rs")" -gt 0 ]; then
  ok "A4 an admin-side writer for email_verified exists, routed and implemented"
else
  bad "A4 no administrator can mark an address verified — the only exits from the lockout need the mail that is broken"
fi

# A5. The listing must tell the operator WHICH accounts are beyond self-rescue.
UNV=$(fn_body "$BE/routes/security.rs" unverified_users)
if [ -n "$UNV" ] && [ "$(printf '%s' "$UNV" | grep -c 'email_token IS NOT NULL')" -gt 0 ]; then
  ok "A5 the unverified listing discriminates accounts that hold no token from those with a live link"
else
  bad "A5 the unverified listing does not say which accounts can never verify themselves"
fi

# A6. Documented recovery. The one lockout a user actually hit was the only one in
# this guide with neither an override nor a documented way back.
if [ "$(grep -rc 'email_verified' docs/guides/security-hardening.md)" -gt 0 ] \
   && [ "$(grep -rc 'Email not verified' docs/guides/security-hardening.md)" -gt 0 ]; then
  ok "A6 the guide documents the 'Email not verified' lockout and how to clear it"
else
  bad "A6 the email-verification lockout is undocumented — no symptom, no recovery, in the hardening guide"
fi

# A7 — CONTROL, green at BOTH tags. Fixing the lockout must not have removed the
# gate. If this ever goes red the 'fix' deleted a security check.
LOGIN=$(fn_body "$BE/routes/auth.rs" login)
if [ -n "$LOGIN" ] && [ "$(printf '%s' "$LOGIN" | grep -c 'email_verified')" -gt 0 ]; then
  ok "A7 CONTROL login still refuses an unverified account — the gate was repaired, not removed"
else
  bad "A7 CONTROL login no longer consults email_verified — the lockout was 'fixed' by deleting the check"
fi

echo "── §B the inert panel-whitelist chain is gone ──"

# B1/B2. The chain and its file must have no references left in shipped source.
# Scoped to panel/ so the CHANGELOG's historical entries are not matched.
chain=$(grep -rl 'panel-whitelist' panel/ --include='*.rs' --include='*.tsx' --include='*.ts' 2>/dev/null | grep -v '/dist/' | grep -v node_modules | wc -l)
if [ "$chain" -eq 0 ]; then
  ok "B1 no shipped source references the panel-whitelist endpoint chain"
else
  bad "B1 $chain shipped source file(s) still reference panel-whitelist — a control that writes a file nothing reads"
fi

conf=$(grep -rl 'panel-whitelist\.conf' panel/ scripts/ --include='*.rs' --include='*.sh' --include='*.tsx' 2>/dev/null | grep -v '/dist/' | wc -l)
if [ "$conf" -eq 0 ]; then
  ok "B2 nothing writes or reads /etc/dockpanel/panel-whitelist.conf"
else
  bad "B2 $conf file(s) still touch panel-whitelist.conf — the file no enforcement path ever consumed"
fi

# B3 — CONTROL, green at BOTH tags, and the point of the whole section. There were
# TWO allowlists; exactly one was inert. This asserts the ENFORCING one still guards
# every door that mints a session. Red here means the wrong one was deleted.
doors=$(grep -rc 'enforce_panel_ip_allowlist' "$BE"/routes/*.rs 2>/dev/null | awk -F: '{s+=$2} END {print s+0}')
if [ "$doors" -ge 6 ]; then
  ok "B3 CONTROL the enforcing IP allowlist is still wired at $doors sites (helper + every minting door)"
else
  bad "B3 CONTROL the enforcing IP allowlist is referenced only $doors times — the surviving allowlist lost coverage"
fi

echo "── §C the canary reports whether it is actually armed ──"

CAN=$(fn_body "$BE/services/auto_healer.rs" security_check_canary_files)
if [ -z "$CAN" ]; then
  bad "C0 could not slice security_check_canary_files — §C would pass vacuously"
else
  # C1. Absent and unreadable are different facts. They used to share one silent
  # `continue`, which is how a tripwire under ProtectHome=yes reads as all-clear.
  if [ "$(printf '%s' "$CAN" | grep -c 'ErrorKind::NotFound')" -gt 0 ]; then
    ok "C1 the canary check distinguishes a canary that was never created from one it cannot read"
  else
    bad "C1 the canary check treats unreadable exactly like absent — a blind spot reported as all-clear"
  fi

  # C2. And it must say when nothing at all is being watched. Scoped to the
  # function body, so an unrelated 'NOT ARMED' elsewhere in the file cannot satisfy
  # it (#335 — C2's ancestor last session matched a token in a different call).
  if [ "$(printf '%s' "$CAN" | grep -c 'NOT ARMED')" -gt 0 ]; then
    ok "C2 the canary check reports when no canary could be examined at all"
  else
    bad "C2 canary monitoring can be enabled, watch nothing, and say nothing — silence is its only output"
  fi
fi

echo ""
echo "── access-recovery pins: PASS $PASS / FAIL $FAIL ──"
[ "$FAIL" -eq 0 ] || exit 1
