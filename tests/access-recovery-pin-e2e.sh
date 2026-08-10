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
#
# ─────────────────────────────────────────────────────────────────────────────
# s338 — WHAT THIS SUITE MEASURED, AND WHAT IT ONLY APPEARED TO
#
# A regression pin greps RAW source, so PROSE satisfies it. Delete the mechanism,
# leave the explanation behind, and the arm stays green while quoting the comment
# back as its evidence. That is not a hypothesis here: it was executed against
# this suite at HEAD and THREE of its arms were held up by nothing but text.
#
#   A4  Two files, one AND. Turn the admin route line into a comment and turn the
#       handler's declaration into a comment: `passed: 25  failed: 0`. The same
#       two lines BLANKED instead — same deletion, no prose — turned the arm red
#       at once. Only the comment saved it. Worse, the route half could never go
#       red at all: it matched on a substring that a DIFFERENT, unrelated and
#       entirely real route also carries, so half of the AND was decorative.
#
#   B3  The count arm that exists to prove the ENFORCING allowlist survived while
#       the inert one was removed. Comment out all five call sites across three
#       files and the count still reads 6, because the declaration is the sixth
#       and five comments are the rest. A control with the five sites BLANKED was
#       already red before anything was planted — the count is real, and comments
#       alone were propping it up.
#
#   D3  The OAuth approval gate. The body slicer walks brace balance over the raw
#       file, and commenting a block preserves brace balance exactly, so the
#       slice was unchanged and the gate's own SQL was found inside a comment.
#
# The repair is the one proven in the sandbox-paths suite: language-aware
# strippers that BLANK comment lines rather than delete them, defined ABOVE every
# call site rather than in the middle of the file, with every source-reading arm
# routed through them. Sections F–I hold that in place and re-run the mutation on
# every push, because a round of mutation testing done by hand dies with the
# session that did it.
#
# TWO THINGS THAT ARE DELIBERATE AND LOOK LIKE OMISSIONS:
#
#   * The arms that read `docs/guides/security-hardening.md` read it RAW. That
#     file IS prose; a markdown heading begins with `#` and stripping there would
#     eat the very sentences those arms measure. Marked at each site.
#
#   * Stripping makes a check see LESS, and that fails in two opposite ways. For a
#     POSITIVE arm (match ⇒ ok) an over-strip is loudly red. For a NEGATIVE arm
#     (match ⇒ bad) an over-strip is SILENTLY GREEN on broken code — the direction
#     that ships. This suite has six negative arms and §H gives each one a control
#     that writes the real defect into a copy and requires the grep to FIRE.
# ─────────────────────────────────────────────────────────────────────────────

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# ── the comment strippers, ABOVE every arm that needs them (s338) ────────────
#
# A check placed below the thing it must run before inherits that position as a
# defect even when the check itself is perfect. The sandbox-paths suite defined
# these at line 379 and every call site above it read raw source; two sibling
# suites shipped the same shape. So they are the first executable thing here.
#
# The comment marker is chosen by EXTENSION, never guessed. Getting it wrong is
# not symmetric: over-stripping turns a positive arm loudly red, but turns a
# NEGATIVE arm (match ⇒ bad) silently green, which is the direction that ships.
comment_marker_for() {
  case "$1" in
    *.rs|*.mjs|*.js|*.ts|*.tsx)                echo '//' ;;
    *.sql)                                     echo '--' ;;  # SQL line comment
    *.json|*.md)                               echo ''   ;;  # no comment syntax; `#` is a heading
    *)                                         echo '#'  ;;  # .sh, .service, .gitignore, .toml
  esac
}

# BLANK the comment, do not delete it. Line numbers and brace balance are both
# read out of this stream; a stripper that removes lines renumbers every subject
# and silently changes what those readers measure.
code_lines() {
  local f="$1" m
  # Absence is not silence: every caller has a non-vacuity floor above it (§G
  # asserts each pinned pattern is present in CODE, §A/§D refuse to report on an
  # empty enumeration), and the preflight below refuses to start without them.
  [ -f "$f" ] || return 0
  m=$(comment_marker_for "$f")
  if [ -z "$m" ]; then cat "$f"; return; fi
  if [ "$m" = '//' ]; then
    # A block comment is recognised ONLY where one is actually written: opening
    # at the start of a line and closing at the end of one. s294: a `/*` inside a
    # string literal (a Dockerfile's `COPY … /app/target/release/*`) opened a
    # comment that swallowed 485 lines of git_build.rs, and every ABSENCE arm
    # over it passed on code the stripper had merely removed.
    #
    # A trailing `//` is stripped, but only when the text before it has BALANCED
    # double quotes — otherwise `let u = "https://host"` would lose its tail.
    # Backslash escapes are skipped so `\"` does not flip the parity.
    awk '
      /^[[:space:]]*\/\*/ &&  /\*\/[[:space:]]*$/ { print ""; next }
      /^[[:space:]]*\/\*/                         { inblk = 1; print ""; next }
      inblk { if ($0 ~ /\*\/[[:space:]]*$/) inblk = 0; print ""; next }
      /^[[:space:]]*\/\//                         { print ""; next }
      {
        line = $0; out = line; n = length(line); q = 0; i = 1
        while (i <= n) {
          c = substr(line, i, 1)
          if (c == "\\") { i += 2; continue }
          if (c == "\"") { q = 1 - q; i++; continue }
          if (q == 0 && c == "/" && substr(line, i + 1, 1) == "/") { out = substr(line, 1, i - 1); break }
          i++
        }
        print out
      }
    ' "$f"
  else
    # FULL-LINE comments only, and the asymmetry with `//` above is deliberate.
    # A trailing marker cannot be stripped safely for either of these languages:
    # `${VAR#prefix}`, `$#`, a colour literal and `sed 's/#//'` are ordinary
    # shell, and `'a--b'` is an ordinary SQL literal. The convention in this tree
    # is that explanation goes on its own line, and §F asserts the stripper
    # leaves a marker inside a string alone. Matched with index() rather than a
    # regex so the marker is never read as a metacharacter.
    awk -v m="$m" '{ t = $0; sub(/^[[:space:]]+/, "", t); if (index(t, m) == 1) { print ""; next } print }' "$f"
  fi
}

# Counted, never `grep -q`, and the reason is a trap this suite already walked
# into once (#334). Under `set -o pipefail`, `code_lines f | grep -q PATTERN`
# reports FAILURE on a successful match: grep -q exits at the first hit, the
# producer upstream dies of SIGPIPE (141), and pipefail takes the pipeline's
# status from it. The effect is silent and selective — a file whose match is near
# the top gets dropped while one whose match is near the bottom survives. `grep
# -c` consumes all of its input, so there is no early exit and no signal.
code_count() { local f="$1"; shift; code_lines "$f" | grep -c "$@" 2>/dev/null || true; }
has_code()   { local f="$1"; shift; [ "$(code_count "$f" "$@")" -gt 0 ]; }

# True file line numbers, because code_lines blanks rather than deletes.
code_lineno() { local f="$1"; shift; code_lines "$f" | grep -n "$@" 2>/dev/null | head -1 | cut -d: -f1 || true; }

# The DELIBERATE raw read, and the only one. Three arms below must see prose:
# A6/E3 measure sentences in a markdown guide, and F11 compares raw against code on a
# real repository file — a comparison that says nothing if both sides are stripped.
# Naming it means §I's scan reports one auditable place rather than a habit, and means
# a future arm cannot reach raw source by accident and look like these three.
raw_count() { local f="$1"; shift; grep -c "$@" "$f" 2>/dev/null || true; }

BE=panel/backend/src
AG=panel/agent/src
FE=panel/frontend/src

# Read RAW on purpose, everywhere it appears below: this is a markdown document,
# every heading in it begins with the shell comment marker, and a stripper aimed
# at it would blank the prose these arms exist to measure.
GUIDE=docs/guides/security-hardening.md

for d in "$BE" "$AG" "$FE" panel/backend/migrations docs; do
  [ -d "$d" ] || { echo "missing dir: $d — refusing to report a clean sweep"; exit 1; }
done

# Fail at the door rather than through an empty stream. code_lines returns
# nothing for a file that is not there, and "nothing" reads exactly like "no
# match" at every call site.
for f in "$BE/routes/auth.rs" "$BE/routes/oauth.rs" "$BE/routes/passkeys.rs" \
         "$BE/routes/mod.rs" "$BE/routes/security.rs" "$BE/routes/users.rs" \
         "$BE/routes/reseller_dashboard.rs" "$BE/routes/whmcs.rs" \
         "$BE/services/activity.rs" "$BE/services/auto_healer.rs" \
         "$AG/routes/security.rs" "$GUIDE"; do
  [ -f "$f" ] || { echo "missing file: $f — refusing to report a clean sweep"; exit 1; }
done

# NOTE ON WINDOWS: no arm below uses a fixed `grep -A n`. A fixed forward window is
# not a block and rots the moment somebody writes a comment above the subject
# (#333 — my own 14-line explanation silently unhooked an arm at s331). Bodies are
# sliced to the construct's own terminator, and always out of the STRIPPED stream:
# strip first, slice second. Blanking preserves height and brace balance, so every
# slice boundary lands exactly where it did before — while a commented-out block
# stops being findable inside the slice, which is the whole point.

# Slice a Rust fn body by brace balance, from `fn <name>` to its matching close.
fn_body() {
  code_lines "$1" | perl -0777 -ne '
    if (/\bfn\s+'"$2"'\b/g) {
      my $s = pos($_); my $i = index($_, "{", $s); my $d = 0;
      for (my $j = $i; $j < length($_); $j++) {
        my $c = substr($_, $j, 1);
        $d++ if $c eq "{"; $d-- if $c eq "}";
        if ($d == 0) { print substr($_, $i, $j - $i + 1); last }
      }
    }'
}

# ── shared predicates ────────────────────────────────────────────────────────
# §H plants a real defect into a throwaway copy and requires each NEGATIVE arm to
# fire on it. A control that reaches its verdict through a different code path
# than the arm it protects proves nothing about that arm, so the predicates live
# here and both the arm and its control call them.

# Every reference to the inert whitelist chain, and to its file, that is CODE.
chain_refs() { code_count "$1" -F -e 'panel-whitelist'; }
conf_refs()  { code_count "$1" -F -e 'panel-whitelist.conf'; }

# Every `INSERT INTO users` statement in one file, flattened to one line each and
# tagged with the file it came from.
inserts_in() {
  code_lines "$1" | perl -0777 -ne '
    while (/INSERT INTO users([^"]*)/gs) { my $s = $1; $s =~ s/\s+/ /g; print "'"$1"'\t$s\n" }'
}

# The audit-retention block, sliced by brace balance from the directory constant.
# The heading that used to anchor this slice is a comment, and a comment is
# exactly what this stream no longer contains — so the anchor is the code.
retention_window() {
  code_lines "$1" | perl -0777 -ne '
    if (/audit_dir = "\/var\/lib\/dockpanel\/audit"/g) {
      my $s = pos($_); my $i = index($_, "{", $s); my $d = 0;
      for (my $j = $i; $j < length($_); $j++) {
        my $c = substr($_, $j, 1);
        $d++ if $c eq "{"; $d-- if $c eq "}";
        if ($d == 0) { print substr($_, $i, $j - $i + 1); last }
      }
    }'
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
# The migration is nine-tenths `--` commentary explaining the predicate, and that
# commentary spells the predicate. Counted in CODE, so the explanation cannot
# stand in for the statement.
MIG=""
for m in panel/backend/migrations/*.sql; do
  [ "$(code_count "$m" -F -e 'email_token IS NULL')" -gt 0 ] || continue
  [ "$(code_count "$m" -F -e 'email_verified')" -gt 0 ]     || continue
  MIG="$m"; break
done
if [ -n "$MIG" ]; then
  ok "A3 a migration releases accounts that are unverified AND tokenless ($(basename "$MIG"))"
else
  bad "A3 no migration releases already-installed accounts that can never verify — the fix reaches new installs only (#332)"
fi

# A4. An administrator must be able to do it on a running panel. `email_verified`
# had five writers and every one was self-service or machine.
#
# Both halves are counted in CODE, and the route half is anchored on the HANDLER
# rather than on the path. The old anchor was a path fragment that a different,
# entirely real and unrelated route in the same file also carries, so that half of
# the AND could not go red whatever happened to the admin route — the arm was one
# assertion wearing two.
if [ "$(code_count "$BE/routes/mod.rs" -F -e 'security::verify_user_email')" -gt 0 ] \
   && [ "$(code_count "$BE/routes/security.rs" -E -e 'fn verify_user_email')" -gt 0 ]; then
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
# RAW BY DESIGN: the subject is prose. See the note on GUIDE and raw_count above.
if [ "$(raw_count "$GUIDE" -F -e 'email_verified')" -gt 0 ] \
   && [ "$(raw_count "$GUIDE" -F -e 'Email not verified')" -gt 0 ]; then
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

# B1/B2 are NEGATIVE arms: a match means the defect is back. A raw grep here would
# go red on a tombstone comment that is not a reference at all, and — the direction
# that matters — a stripper that removed too much would report the chain gone while
# it was still compiled in. §H plants the real thing and requires both to fire.
#
# The candidate list comes from a RAW grep and is then confirmed in CODE. That is
# not a shortcut: code is a subset of raw, so the raw pass can only ever be a
# superset, and it keeps this off a full walk of the frontend tree.

CHAIN_CANDS=$(grep -rl 'panel-whitelist' panel/ --include='*.rs' --include='*.tsx' --include='*.ts' 2>/dev/null \
              | grep -v '/dist/' | grep -v node_modules || true)
chain=0
for f in $CHAIN_CANDS; do
  if [ "$(chain_refs "$f")" -gt 0 ]; then chain=$((chain+1)); fi
done
if [ "$chain" -eq 0 ]; then
  ok "B1 no shipped source references the panel-whitelist endpoint chain"
else
  bad "B1 $chain shipped source file(s) still reference panel-whitelist — a control that writes a file nothing reads"
fi

CONF_CANDS=$(grep -rl 'panel-whitelist\.conf' panel/ scripts/ --include='*.rs' --include='*.sh' --include='*.tsx' 2>/dev/null \
             | grep -v '/dist/' || true)
conf=0
for f in $CONF_CANDS; do
  if [ "$(conf_refs "$f")" -gt 0 ]; then conf=$((conf+1)); fi
done
if [ "$conf" -eq 0 ]; then
  ok "B2 nothing writes or reads /etc/dockpanel/panel-whitelist.conf"
else
  bad "B2 $conf file(s) still touch panel-whitelist.conf — the file no enforcement path ever consumed"
fi

# B3 — CONTROL, green at BOTH tags, and the point of the whole section. There were
# TWO allowlists; exactly one was inert. This asserts the ENFORCING one still guards
# every door that mints a session. Red here means the wrong one was deleted.
#
# Counted in CODE. Commenting out all five call sites across three files left this
# reading 6 and green, because the declaration is the sixth reference and five
# comments made up the rest — the arm was measuring text, not wiring.
doors=0
for f in "$BE"/routes/*.rs; do
  doors=$(( doors + $(code_count "$f" -F -e 'enforce_panel_ip_allowlist') ))
done
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

echo "── §D s333: the OTHER doors that create an account, and the doors that admit one ──"

# WHY §D EXISTS. §A above pinned ONE door — `auth::setup` — because that is the door
# the reporter hit. The enumeration that convinced me the others were fine grepped for
# the COLUMN NAME, and a name-grep is structurally blind to an INSERT that omits the
# column: the two doors that were still broken (`users::create` and
# `reseller_dashboard::create_user`) are exactly the two files that never spell it.
# §A was green on both of them for a whole release. So D1 asks the question in the
# only order that can work — find every writer FIRST, then ask each one about the
# column (#341: an arm must ask whether EVERY member has the property, never pick a
# member and test it).
#
# The enumeration reads CODE. §D searches for a SQL statement by its text, and prose
# that merely NARRATES the statement is not a statement — there is a doc comment in
# services/activity.rs that spells the statement while describing it, and counting
# that as a door would fail this suite on a file that has no door in it. §F11 pins
# that exact file as the live proof the stripper is doing it.

INSERTS=$(for f in $(grep -rl 'INSERT INTO users' "$BE" --include=*.rs 2>/dev/null); do   # RAW-ENUM-OK: discovery, not an assertion; each hit is re-read through inserts_in
  inserts_in "$f"
done)
n_ins=$(printf '%s' "$INSERTS" | grep -c . || true)

# D2 FIRST, because every other arm here is worthless if the enumeration is empty. An
# arm that enumerates its own subjects must prove the list is real before reporting on
# it (#143 — two absence arms once printed green having examined zero files).
if [ "$n_ins" -ge 5 ]; then
  ok "D2 the account-writer enumeration found $n_ins INSERT INTO users statements — non-vacuous"
else
  bad "D2 only $n_ins INSERT INTO users statements found (expected >= 5) — the extractor is broken and every §D arm below is meaningless"
fi

# D1. Each writer must either mark the account verified, or issue it a token so the
# holder can verify themselves. Neither means an account that cannot sign in once SMTP
# exists and cannot ever fix that — the #100 predicate, which HEAD's own migration
# defines as `email_verified = FALSE AND email_token IS NULL`.
#
# NEGATIVE: green means the offender list is EMPTY. An over-strip that dropped a real
# offending door out of the enumeration would report exactly that, so §H plants a door
# with neither column and requires this predicate to name it.
offenders=$(printf '%s\n' "$INSERTS" | grep -v 'email_verified' | grep -v 'email_token' \
            | cut -f1 | sort -u | tr '\n' ' ' | sed 's/ *$//')
if [ "$n_ins" -ge 5 ] && [ -z "$offenders" ]; then
  ok "D1 every door that creates an account either verifies it or issues it a verification token"
else
  bad "D1 these doors create accounts that can never verify and can never sign in once SMTP is set: ${offenders:-<enumeration failed>} (#100, in a door nobody reported)"
fi

# D5 CONTROL — green at both tags. D1 must not be satisfiable by making everything
# verified: the self-service registration door is SUPPOSED to leave the account
# unverified, and it earns that by issuing a token. If this ever goes red alongside a
# green D1, someone "fixed" D1 by deleting the verification ceremony.
REG_BODY=$(fn_body "$BE/routes/auth.rs" register)
reg_ins=$(printf '%s' "$REG_BODY" | perl -0777 -ne 'print $1 if /(INSERT INTO users[^"]*)/s')
if [ -n "$reg_ins" ] && [ "$(printf '%s' "$reg_ins" | grep -c 'email_token')" -gt 0 ]; then
  ok "D5 (control) self-service registration still issues a verification token rather than self-approving"
else
  bad "D5 (control) the registration door no longer issues an email_token — the verification ceremony was removed, not repaired"
fi

# D3/D4/D6. THE DOORS THAT ADMIT A SESSION. Three of them exist and they did not agree:
# the password and passkey doors refuse an unapproved account, the OAuth door did not
# check at all, so Registration Approval Mode was enforced at two ways in out of three.
# The same file's own comment claims parity with the other two — for SUSPENSION, which
# it does have. Getting one of two parities right is the tell (#338: when two doors do
# the same job, diff them; the asymmetry is the bug).
#
# And the comment that claims the parity sits directly above the gate that provides it.
# Commenting the seven lines of the OAuth gate out left this arm green: brace balance
# is preserved by commenting, so the slice was identical and the gate's own SQL was
# found in the prose. The slice now comes off the blanked stream, where it is not.
for door in "routes/auth.rs:login" "routes/passkeys.rs:auth_complete" "routes/oauth.rs:callback"; do
  df=${door%%:*}; dn=${door##*:}
  BODY=$(fn_body "$BE/$df" "$dn")
  if [ -z "$BODY" ]; then
    bad "D3 could not slice $dn out of $df — the gate-parity arms would pass vacuously"
    continue
  fi
  if [ "$(printf '%s' "$BODY" | grep -c 'COALESCE(approved')" -gt 0 ]; then
    ok "D3 $df::$dn refuses an account pending admin approval"
  else
    bad "D3 $df::$dn admits a session without checking approval — Registration Approval Mode is bypassable through this door"
  fi
  # D6 CONTROL — green at both tags. Proves the extractor really is reading these three
  # bodies, and that adding the approval gate did not disturb the suspension gate.
  if [ "$(printf '%s' "$BODY" | grep -c '"suspended"')" -gt 0 ]; then
    ok "D6 (control) $df::$dn still refuses a suspended account"
  else
    bad "D6 (control) $df::$dn no longer refuses suspended accounts"
  fi
done

OAUTH_CB=$(fn_body "$BE/routes/oauth.rs" callback)
if [ -z "$OAUTH_CB" ]; then
  bad "D4 could not slice oauth::callback"
elif [ "$(printf '%s' "$OAUTH_CB" | grep -c 'security_approval_required')" -gt 0 ]; then
  ok "D4 an OAuth signup honours Registration Approval Mode instead of admitting itself"
else
  bad "D4 an OAuth signup ignores Registration Approval Mode: the column defaults TRUE, so the account is admitted at once and never appears in Approvals (that list selects approved = FALSE)"
fi

echo ""
echo "── §E s333: the audit log claims only what it enforces ──"

# E1. The uncalled one-shot initialiser is gone. Its only unique action set the
# append-only attribute on the audit DIRECTORY, which permits rewriting the files in
# place and denies the panel's own retention sweep. ⚠ The literal is deliberately NOT
# spelled anywhere in the shipped agent source, including in the tombstone comment
# there — that is what turned this exact arm red on a correct fix one session ago
# (#340). Build it here instead so the arm and the prohibition live together.
#
# NEGATIVE, and counted in CODE: a tombstone comment naming the route is not a mount,
# and an over-strip would report the mount gone while it was still there. §H plants it.
NEEDLE="/security/$(printf 'init')"
if [ "$(code_count "$AG/routes/security.rs" -F -e "$NEEDLE")" -eq 0 ]; then
  ok "E1 the uncalled security-hardening initialiser is gone from the agent's shipped routes"
else
  bad "E1 the agent still mounts the one-shot initialiser whose only unique action mis-places the append-only attribute"
fi

# E2. The retention sweep must not discard its delete result. Under the append-only
# attribute unlink is refused even for root, so a discarded Result meant retention
# could fail forever and report exactly what a successful sweep reports: nothing.
#
# The block used to be sliced on the heading comment above it, which is the one thing
# a comment-stripped stream cannot contain. It is anchored on the directory constant
# now — code, and the only code in the function that names this directory.
RET=$(retention_window "$BE/services/auto_healer.rs")
if [ -n "$RET" ] && [ "$(printf '%s' "$RET" | grep -c 'let _ = std::fs::remove_file')" -eq 0 ] \
   && [ "$(printf '%s' "$RET" | grep -c 'audit_failed')" -gt 0 ]; then
  ok "E2 the audit retention sweep reports a delete it could not perform instead of discarding it"
else
  bad "E2 the audit retention sweep discards its delete result — under an append-only directory it fails forever and says nothing"
fi

# E3. The guide must not promise on-disk tamper-proofing that nothing establishes.
# The sentence claimed append-only FILES while the only code set the attribute on the
# directory, which does not prevent rewriting them.
# RAW BY DESIGN: the subject is prose, and the assertion is ABOUT the prose.
if [ "$(raw_count "$GUIDE" -F -e 'append-only files on disk')" -eq 0 ] \
   && [ "$(raw_count "$GUIDE" -F -e 'chattr +a')" -gt 0 ]; then
  ok "E3 the hardening guide states what the audit log actually enforces, and how an operator can add the rest"
else
  bad "E3 the hardening guide still promises append-only files on disk, which no shipped code establishes"
fi

echo ""
echo "── §F the strippers strip comments, and nothing else (s338) ──"

# Everything above now depends on code_lines. A stripper that under-strips restores
# the whole defect silently; one that OVER-strips turns §B, §D1 and §E's negative arms
# green on broken code. So it gets its own fixtures, and they are hermetic — no
# repository file is involved, because a fixture that shares a subject with the thing
# it validates cannot tell the two apart.
FIXDIR=$(mktemp -d)
trap 'rm -rf "$FIXDIR"' EXIT

cat > "$FIXDIR/f.sh" <<'FIX'
#!/usr/bin/env bash
# TOKENWORD lives here as a full-line comment
run_the TOKENWORD --now
echo "a literal hash # inside a string keeps TOKENSTR"
FIX

cat > "$FIXDIR/f.rs" <<'FIX'
// TOKENWORD in a line comment
/*
 * TOKENWORD in a block comment
 */
let live = call(TOKENWORD);
let trail = other(); // TOKENWORD trailing
let url = "https://example.invalid/TOKENURL";
let glob = "COPY /app/target/release/*";
let after_glob = keep(TOKENAFTER);
FIX

cat > "$FIXDIR/f.sql" <<'FIX'
-- TOKENWORD in a SQL line comment
UPDATE users SET note = 'x' WHERE tag = 'TOKENWORD';
UPDATE users SET note = 'a--b TOKENDASH' WHERE id = 1;
FIX

cat > "$FIXDIR/f.md" <<'FIX'
# TOKENWORD in a markdown heading
Some prose about TOKENWORD.
FIX

printf '{ "name": "x", "note": "TOKENWORD" }\n' > "$FIXDIR/f.json"

if [ "$(code_count "$FIXDIR/f.sh" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "F1 shell: a full-line '#' comment carrying the token is blanked (1 code hit, not 2)"
else
  bad "F1 shell: expected exactly 1 code hit for TOKENWORD, got $(code_count "$FIXDIR/f.sh" -F -e 'TOKENWORD')"
fi

if [ "$(code_count "$FIXDIR/f.sh" -F -e 'TOKENSTR')" -eq 1 ]; then
  ok "F2 shell: a '#' inside a string literal is left alone (no trailing-comment stripping)"
else
  bad "F2 shell: the stripper ate a '#' inside a string — a negative arm would now pass on broken code"
fi

if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "F3 rust: '//' line, '/* */' block and trailing '//' all blanked (1 code hit of 4)"
else
  bad "F3 rust: expected exactly 1 code hit for TOKENWORD, got $(code_count "$FIXDIR/f.rs" -F -e 'TOKENWORD')"
fi

if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENURL')" -eq 1 ]; then
  ok "F4 rust: '//' inside a quoted string survives — a URL is not a comment"
else
  bad "F4 rust: the stripper cut a line at the '//' of a URL inside a string literal"
fi

# s294: a `/*` inside a string literal opened a block comment that ran to the next
# `*/` and deleted 485 lines of git_build.rs. The guard is that a block is recognised
# only where one is WRITTEN — opening at line start.
if [ "$(code_count "$FIXDIR/f.rs" -F -e 'TOKENAFTER')" -eq 1 ]; then
  ok "F5 rust: a '/*' inside a string literal does not open a block (the s294 guard holds)"
else
  bad "F5 rust: a string containing '/*' swallowed the code after it — the s294 stripper defect is back"
fi

if [ "$(code_count "$FIXDIR/f.json" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "F6 json: nothing is stripped, because JSON has no comment syntax"
else
  bad "F6 json: the stripper altered a JSON file, which has no comments to remove"
fi

# The migration §A3 reads is nine-tenths commentary and that commentary spells the
# predicate. `#` would have stripped none of it and the arm would still be reading
# prose; `--` is the marker the language actually uses.
if [ "$(code_count "$FIXDIR/f.sql" -F -e 'TOKENWORD')" -eq 1 ]; then
  ok "F7 sql: a full-line '--' comment carrying the token is blanked (1 code hit, not 2)"
else
  bad "F7 sql: expected exactly 1 code hit for TOKENWORD, got $(code_count "$FIXDIR/f.sql" -F -e 'TOKENWORD') — a migration's own commentary would satisfy A3"
fi

if [ "$(code_count "$FIXDIR/f.sql" -F -e 'TOKENDASH')" -eq 1 ]; then
  ok "F8 sql: a '--' inside a quoted literal is left alone (full-line comments only)"
else
  bad "F8 sql: the stripper ate a '--' inside a string literal"
fi

# The hardening guide is read RAW by A6 and E3, and this is why: its headings begin
# with the shell comment marker. If the dispatch ever sends markdown to that branch,
# every heading in the document disappears and both arms start measuring nothing.
if [ "$(code_count "$FIXDIR/f.md" -F -e 'TOKENWORD')" -eq 2 ]; then
  ok "F9 markdown: nothing is stripped — a '#' heading is prose, not a comment"
else
  bad "F9 markdown: the stripper treated a heading as a comment ($(code_count "$FIXDIR/f.md" -F -e 'TOKENWORD') of 2 hits left) — A6 and E3 would go blind"
fi

# Blanking, not deleting. §I reads a line number out of this stream and the two body
# slicers depend on the height and brace balance of the original being preserved.
LINEDRIFT=""
for f in "$BE/routes/auth.rs" "$BE/routes/oauth.rs" "$BE/routes/passkeys.rs" \
         "$BE/routes/mod.rs" "$BE/routes/security.rs" "$BE/routes/users.rs" \
         "$BE/routes/reseller_dashboard.rs" "$BE/routes/whmcs.rs" \
         "$BE/services/activity.rs" "$BE/services/auto_healer.rs" \
         "$AG/routes/security.rs" "$GUIDE" tests/access-recovery-pin-e2e.sh; do
  a=$(wc -l < "$f"); b=$(code_lines "$f" | wc -l)
  [ "$a" != "$b" ] && LINEDRIFT="$LINEDRIFT $f($a≠$b)"
done
if [ -z "$LINEDRIFT" ]; then
  ok "F10 blanking preserves the line count of every subject, so line numbers and brace balance stay true"
else
  bad "F10 code_lines changed the line count of:$LINEDRIFT — the body slicers and the §I ordering check are measuring the wrong lines"
fi

# The live proof, in the repository rather than in a fixture. services/activity.rs
# NARRATES the statement §D enumerates and contains no such statement. Raw, it is a
# seventh door; stripped, it is what it is.
act_raw=$(raw_count "$BE/services/activity.rs" -F -e 'INSERT INTO users')
act_code=$(code_count "$BE/services/activity.rs" -F -e 'INSERT INTO users')
if [ "$act_raw" -gt 0 ] && [ "$act_code" -eq 0 ]; then
  ok "F11 services/activity.rs spells the enumerated statement $act_raw time(s) in prose and 0 in code — the stripper is doing this on the real tree, not only on fixtures"
else
  bad "F11 activity.rs raw=$act_raw code=$act_code — either the prose mention is gone (retire this arm deliberately) or the stripper is not reaching it and §D is counting a comment as a door"
fi

echo ""
echo "── §G every pinned pattern is live CODE, and a comment cannot satisfy it (s338) ──"

# This is the s338 mutation, encoded. A round of mutation testing done by hand dies
# with the session that did it; the arms below re-run it on every push.
#
# For each subject: assert every pattern this suite pins is present in CODE (a pattern
# that has quietly become prose-only would otherwise turn red later and look like a
# test bug rather than the defect it is), then turn each matching code line into a
# comment in a throwaway copy and assert the count falls to zero. If it does not, some
# arm above is still reading the file raw.
MIG_PIN=${MIG:-panel/backend/migrations/__A3_found_no_migration__.sql}

PINNED=(
  "$BE/routes/auth.rs|-F|INSERT INTO users"
  "$BE/routes/auth.rs|-F|email_verified"
  "$BE/routes/auth.rs|-F|email_token"
  "$BE/routes/auth.rs|-F|enforce_panel_ip_allowlist"
  "$BE/routes/auth.rs|-F|COALESCE(approved"
  "$BE/routes/auth.rs|-F|\"suspended\""
  "$BE/routes/passkeys.rs|-F|enforce_panel_ip_allowlist"
  "$BE/routes/passkeys.rs|-F|COALESCE(approved"
  "$BE/routes/passkeys.rs|-F|\"suspended\""
  "$BE/routes/oauth.rs|-F|enforce_panel_ip_allowlist"
  "$BE/routes/oauth.rs|-F|COALESCE(approved"
  "$BE/routes/oauth.rs|-F|\"suspended\""
  "$BE/routes/oauth.rs|-F|security_approval_required"
  "$BE/routes/mod.rs|-F|security::verify_user_email"
  "$BE/routes/security.rs|-E|fn verify_user_email"
  "$BE/routes/security.rs|-F|email_token IS NOT NULL"
  "$BE/services/auto_healer.rs|-F|ErrorKind::NotFound"
  "$BE/services/auto_healer.rs|-F|NOT ARMED"
  "$BE/services/auto_healer.rs|-F|audit_failed"
  "$BE/services/auto_healer.rs|-F|audit_dir = \"/var/lib/dockpanel/audit\""
  "$BE/routes/users.rs|-F|INSERT INTO users"
  "$BE/routes/reseller_dashboard.rs|-F|INSERT INTO users"
  "$BE/routes/whmcs.rs|-F|INSERT INTO users"
  "$MIG_PIN|-F|email_token IS NULL"
  "$MIG_PIN|-F|email_verified"
)

# Turn every CODE line matching the pattern into a comment, leaving the text. That is
# the defect this section is about: the mechanism removed, the explanation left behind.
#
# The copy KEEPS the original's extension, and that is load-bearing rather than tidy:
# comment_marker_for dispatches on the extension, so a scratch file called `mutated` is
# read as shell no matter what it holds — the `//` and `--` subjects would report the
# stripper broken when what was broken was the temp filename.
comment_out_matches() {
  local src="$1" dst="$2" mode="$3" pat="$4" marker
  marker=$(comment_marker_for "$src")
  code_lines "$src" | grep -n "$mode" -e "$pat" 2>/dev/null | cut -d: -f1 > "$FIXDIR/hits" || true
  # Keyed on FILENAME, not `NR == FNR`: with an EMPTY hit list (a pin that has gone
  # prose-only) the first record of the SUBJECT also satisfies NR == FNR, the whole
  # file is consumed as hit numbers, and the empty copy then reads as "the stripper
  # is not reaching this subject" when the truth is "nothing left to comment out".
  awk -v marker="$marker" -v hitsfile="$FIXDIR/hits" \
      'FILENAME == hitsfile { hit[$1]=1; next } { if (FNR in hit) print marker " " $0; else print }' \
      "$FIXDIR/hits" "$src" > "$dst"
}

PIN_SUBJECTS=$(printf '%s\n' "${PINNED[@]}" | cut -d'|' -f1 | sort -u)
for subj in $PIN_SUBJECTS; do
  dead="" survived=""
  for entry in "${PINNED[@]}"; do
    f=${entry%%|*}; rest=${entry#*|}; mode=${rest%%|*}; pat=${rest#*|}
    [ "$f" = "$subj" ] || continue
    [ "$(code_count "$f" "$mode" -e "$pat")" -gt 0 ] || dead="$dead [$pat]"
    mutated="$FIXDIR/mutated_$(basename "$f")"
    comment_out_matches "$f" "$mutated" "$mode" "$pat"
    # Assert the plant LANDED before reading the result: an empty copy would score
    # zero and read exactly like a stripper that works.
    if [ ! -s "$mutated" ]; then
      survived="$survived [$pat→copy-empty]"
      continue
    fi
    left=$(code_lines "$mutated" | grep -c "$mode" -e "$pat" 2>/dev/null || true)
    [ "$left" -eq 0 ] || survived="$survived [$pat→$left]"
  done

  if [ -z "$dead" ]; then
    ok "G $subj: every pattern this suite pins is present in CODE, not only in prose"
  else
    bad "G $subj: pinned pattern(s) present only as prose —$dead — the arm reading it is passing on a comment"
  fi

  if [ -z "$survived" ]; then
    ok "G $subj: commenting out the matching code lines drives every pinned pattern to zero"
  else
    bad "G $subj: pattern(s) still counted after their code lines were commented out —$survived — the stripper is not reaching this subject"
  fi
done

echo ""
echo "── §H the six NEGATIVE arms still fire on a real defect (s338) ──"

# Stripping makes a check see LESS, and that is not a symmetric risk. A positive arm
# that loses its subject goes loudly red at HEAD. A NEGATIVE arm — one where a match
# means the defect is present — goes SILENTLY GREEN, and a suite reporting 0 failures
# on broken code is the exact failure this whole retrofit exists to prevent.
#
# So each negative arm gets the defect written into a throwaway copy, IN CODE, and is
# required to notice. Each control calls the same predicate function the arm calls, so
# a control cannot pass down a path the arm does not use.

# H1 — B1. A live reference to the inert chain, in code.
cat > "$FIXDIR/wl.rs" <<'FIX'
// a tombstone naming panel-whitelist must NOT count as a reference
pub async fn save_panel_whitelist() -> Json<Value> {
    let path = build_route("/api/security/panel-whitelist");
    Json(json!({ "ok": true, "path": path }))
}
FIX
if [ "$(chain_refs "$FIXDIR/wl.rs")" -gt 0 ]; then
  ok "H1 B1's chain scan still fires when the panel-whitelist endpoint is really in code"
else
  bad "H1 B1's chain scan no longer sees a real panel-whitelist reference — the stripper is eating code and B1 is green on a live inert control"
fi

# H2 — B2. A live writer of the file nothing consumed.
cat > "$FIXDIR/wlconf.rs" <<'FIX'
// the tombstone may name panel-whitelist.conf; the writer below is the defect
std::fs::write("/etc/dockpanel/panel-whitelist.conf", body)?;
FIX
if [ "$(conf_refs "$FIXDIR/wlconf.rs")" -gt 0 ]; then
  ok "H2 B2's file scan still fires when panel-whitelist.conf is really written in code"
else
  bad "H2 B2's file scan no longer sees a real write of panel-whitelist.conf — B2 is green on a live writer"
fi

# H3 — E1. The one-shot initialiser mounted again. The route literal is assembled from
# the same runtime pieces the arm uses, so it is not spelled in this file either — the
# prohibition that turned E1 red on a correct fix at s340 applies to the test too.
cp "$AG/routes/security.rs" "$FIXDIR/agentsec.rs"
printf '        .route("%s", post(harden_init))\n' "$NEEDLE" >> "$FIXDIR/agentsec.rs"
if [ "$(code_count "$FIXDIR/agentsec.rs" -F -e "$NEEDLE")" -gt 0 ]; then
  ok "H3 E1's absence arm still fires when the one-shot initialiser is really mounted in code"
else
  bad "H3 E1's absence arm no longer sees a real mount of the initialiser — E1 is green on an agent that still mounts it"
fi

# H4 — E2. The retention sweep with its delete result discarded again, sliced by the
# same window function the arm uses.
cat > "$FIXDIR/ret.rs" <<'FIX'
// Clean old audit log files (>365 days) — this narration must not be the anchor
fn sweep() {
    let audit_dir = "/var/lib/dockpanel/audit";
    if let Ok(entries) = std::fs::read_dir(audit_dir) {
        let mut audit_failed: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
FIX
RET_CTL=$(retention_window "$FIXDIR/ret.rs")
if [ -n "$RET_CTL" ] && [ "$(printf '%s' "$RET_CTL" | grep -c 'let _ = std::fs::remove_file')" -gt 0 ]; then
  ok "H4 E2's discarded-result detector still fires when the delete result is really thrown away in code"
else
  bad "H4 E2's window found no discarded delete in a file that has one (window=${#RET_CTL} chars) — E2 is green on a sweep that fails forever in silence"
fi

# H5 — E3. The guide is read RAW by design, so no stripper narrows this one; the
# control exists anyway, because "no stripper touches it" is a claim about today's
# dispatch table and F9 is the only thing holding it.
cp "$GUIDE" "$FIXDIR/guide.md"
printf '\nThe audit log is kept as append-only files on disk.\n' >> "$FIXDIR/guide.md"
if [ "$(raw_count "$FIXDIR/guide.md" -F -e 'append-only files on disk')" -gt 0 ]; then
  ok "H5 E3's overclaim detector still fires when the guide really makes the promise"
else
  bad "H5 E3's overclaim detector no longer sees the promised sentence — E3 is green on a guide that promises what nothing enforces"
fi

# H6 — D1. A door that creates an account with neither column, run through the same
# extractor and the same offender predicate.
cat > "$FIXDIR/door.rs" <<'FIX'
// this door's comment may mention email_verified and email_token freely
let row = sqlx::query_as(
    "INSERT INTO users (email, password_hash, role) \
     VALUES ($1, $2, 'user') RETURNING *",
)
FIX
ctl_off=$(inserts_in "$FIXDIR/door.rs" | grep -v 'email_verified' | grep -v 'email_token' | cut -f1 | sort -u)
if [ -n "$ctl_off" ]; then
  ok "H6 D1's offender predicate still names a door that inserts neither column"
else
  bad "H6 D1's offender predicate found nothing in a door that has neither column — the enumeration or the stripper is dropping real writers and D1 is green on a broken door"
fi

echo ""
echo "── §I no arm in this suite may read a subject raw again (s338) ──"

# The structural guard. §G proves today's arms are comment-blind; this one stops a NEW
# arm from being written the old way, which is exactly how this suite came to have
# three prose-satisfiable assertions in the first place.
SELF=tests/access-recovery-pin-e2e.sh

# Assembled at runtime from a variable, so this line does not contain the shape it
# searches for and the scan cannot match itself. GUIDE is deliberately absent from the
# list: those two arms read prose, on purpose, and F9 is what protects them.
SUBJ_VARS='BE|AG|FE'
# Assembled from two halves so neither scanning line below spells the marker and
# counts itself — the same self-matching hazard SUBJ_VARS is assembled to dodge.
ENUM_HEAD='RAW-ENUM'
ENUM_MARK="$ENUM_HEAD-OK"
RAW_ARMS=$(code_lines "$SELF" | grep -nE "grep[^|;]*\"\\\$($SUBJ_VARS)[\"/]" | grep -v "$ENUM_MARK" || true)
if [ -z "$RAW_ARMS" ]; then
  ok "I1 no arm greps a source subject directly; every read goes through code_lines"
else
  bad "I1 these lines read a source subject raw instead of through code_lines: $(printf '%s' "$RAW_ARMS" | cut -d: -f1 | tr '\n' ' ')"
fi

# I1 scans for `grep`, so naming the deliberate raw read opened a door beside it: a new
# arm could reach raw source through raw_count and be invisible to the guard. Ratchet
# it. Exactly ONE arm may read a source subject raw — F11, which exists to compare raw
# against code and is meaningless if both sides are stripped. The GUIDE calls are not
# counted here: that subject is prose, and F9 is what protects it.
RAW_ON_SOURCE=$(code_lines "$SELF" | grep -cE "raw_count \"\\\$($SUBJ_VARS)/" || true)
if [ "$RAW_ON_SOURCE" -eq 1 ]; then
  ok "I3 exactly one arm reads a source subject raw, and it is the one that compares raw against code"
else
  bad "I3 $RAW_ON_SOURCE arms read a source subject raw (expected exactly 1) — either F11's comparison was removed or a new arm reached raw source past the I1 scan"
fi

# The marker above is an escape hatch, so it is ratcheted like every other one here:
# a file-DISCOVERY grep over-includes (a prose mention lists the file) and each hit is
# then re-read through a stripping helper, which is the safe direction. An arm that
# ASSERTS content must never carry it, and the count is pinned so a second one cannot
# appear unnoticed.
ENUM_OK=$(code_lines "$SELF" | grep -c "$ENUM_MARK" || true)
if [ "$ENUM_OK" -eq 1 ]; then
  ok "I4 exactly one raw read is marked as a discovery enumeration"
else
  bad "I4 $ENUM_OK raw reads are marked as discovery enumerations (expected exactly 1) — the I1 escape hatch has been widened"
fi

# A check placed below the thing it must run before inherits that position as a defect.
# The strippers in the sibling suite were defined at line 379 and used by nothing above
# them. Assert the definition precedes the first section rather than trusting it.
DEF_LINE=$(code_lineno "$SELF" -E -e '^code_lines\(\) \{')
SEC1_LINE=$(code_lineno "$SELF" -F -e '── §A #100')
if [ -n "$DEF_LINE" ] && [ -n "$SEC1_LINE" ] && [ "$DEF_LINE" -lt "$SEC1_LINE" ]; then
  ok "I2 code_lines is defined (line $DEF_LINE) above the first section (line $SEC1_LINE)"
else
  bad "I2 code_lines is no longer defined above §A — the arms above its definition are reading raw source again (def=${DEF_LINE:-none} §A=${SEC1_LINE:-none})"
fi

echo ""
echo "── access-recovery pins: PASS $PASS / FAIL $FAIL ──"
[ "$FAIL" -eq 0 ] || exit 1
