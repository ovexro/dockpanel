#!/usr/bin/env bash
# Regression pins for s275 — the two doors into a panel, and the one that was
# open while the other looked shut.
#
# A panel has more than one way to acquire an account, and until s275 they did
# not agree with each other:
#
#   D1  POST /api/auth/register reads `self_registration_enabled` and defaults
#       CLOSED — an ABSENT row means disabled. It has a visible toggle in
#       Settings → Security Hardening.
#   D2  The OAuth callback auto-creates a user on first sign-in, read
#       `oauth_auto_create`, and defaulted OPEN — an absent row meant ALLOWED.
#       It had no control anywhere in the panel, though it was writable through
#       the settings API.
#
# Both rows are absent on a fresh install. So the panel's answer to "is
# registration open?" depended on which door you knocked on, and an operator who
# switched the visible toggle off and later configured a provider — a normal
# thing to do — had self-registration silently back on through a switch they
# could not see.
#
# Found by being asked a much simpler question: is registration disabled on the
# demo? It was. The check for it is what surfaced the sibling.
#
# The lesson these pins hold down is not "OAuth needs a gate". It is that a
# control implies a scope, and a second control with the OPPOSITE default and no
# UI silently narrows that scope to nothing.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

AUTH=panel/backend/src/routes/auth.rs
OAUTH=panel/backend/src/routes/oauth.rs
SETTINGS=panel/backend/src/routes/settings.rs
SETTINGS_TSX=panel/frontend/src/pages/Settings.tsx

for f in "$AUTH" "$OAUTH" "$SETTINGS" "$SETTINGS_TSX"; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

echo "── 1. the password door defaults CLOSED ──"

# An absent row must mean disabled. `unwrap_or(true)` here would open every
# panel that has never touched the setting, which is most of them.
if grep -q "key = 'self_registration_enabled'" "$AUTH"; then
  ok "register reads self_registration_enabled"
else
  bad "$AUTH no longer consults self_registration_enabled — the toggle in Settings governs nothing"
fi

if grep -A 6 "key = 'self_registration_enabled'" "$AUTH" | grep -cF 'unwrap_or(false)' >/dev/null; then
  ok "an absent self_registration_enabled row means DISABLED"
else
  bad "$AUTH no longer defaults self-registration closed — a fresh panel would accept public signups with nothing configured"
fi

# Lockdown is the other refusal on this path and it must come FIRST: a panel
# under lockdown that still accepts registrations is worse than one that never
# had the toggle.
#
# Scoped to the REGISTER FUNCTION, not to the file. auth.rs calls
# is_locked_down in three places; the first is in a different function entirely,
# ~300 lines above register. Comparing first-in-file against the registration
# line was true for reasons that had nothing to do with register, and stayed
# green when register's own lockdown check was deleted. A line-ordering
# assertion is only as good as the region it reads.
REGISTER_BODY=$(awk '/^pub async fn register\(/,/^}/' "$AUTH")
LOCK=$(printf '%s\n' "$REGISTER_BODY" | grep -n 'is_locked_down' | head -1 | cut -d: -f1)
SREG=$(printf '%s\n' "$REGISTER_BODY" | grep -n "key = 'self_registration_enabled'" | head -1 | cut -d: -f1)
if [ -z "$REGISTER_BODY" ]; then
  bad "could not isolate the register function in $AUTH — this section is reading nothing"
elif [ -n "$LOCK" ] && [ -n "$SREG" ] && [ "$LOCK" -lt "$SREG" ]; then
  ok "inside register, lockdown is checked (+$LOCK) before the registration setting (+$SREG)"
else
  bad "register does not check lockdown before the registration setting — a locked-down panel would still accept signups"
fi

echo
echo "── 2. the OAuth door honours the SAME answer (s275) ──"

# The fix. An explicit self_registration_enabled=false closes this path too.
# Deliberately the EXPLICIT value, not the effective one: an install that never
# set the row keeps its existing behaviour, so no working OAuth deployment
# breaks on upgrade, and only operators who actually asked for registration to
# be off get what they asked for.
if grep -q "key = 'self_registration_enabled'" "$OAUTH"; then
  ok "the OAuth auto-create path consults self_registration_enabled"
else
  bad "$OAUTH ignores self_registration_enabled — turning Self-Registration off leaves OAuth signup open, which is the s275 defect exactly"
fi

# Ordering: the gate is worth nothing after the INSERT.
GATE=$(grep -n "key = 'self_registration_enabled'" "$OAUTH" | head -1 | cut -d: -f1)
INS=$(grep -n 'INSERT INTO users' "$OAUTH" | head -1 | cut -d: -f1)
if [ -n "$GATE" ] && [ -n "$INS" ] && [ "$GATE" -lt "$INS" ]; then
  ok "the gate (line $GATE) precedes the user INSERT (line $INS)"
else
  bad "$OAUTH's registration gate no longer precedes the INSERT — the account is created before anything refuses"
fi

if grep -q "key = 'oauth_auto_create'" "$OAUTH"; then
  ok "the dedicated oauth_auto_create switch still exists"
else
  bad "$OAUTH no longer reads oauth_auto_create — an operator who wants OAuth logins but not OAuth SIGNUPS has lost the only control for it"
fi

echo
echo "── 3. no DB-only switch: every gate has an operator control ──"

# This is the half that made the defect invisible rather than merely wrong.
# oauth_auto_create was API-writable and had NO control anywhere in the panel,
# so an operator could neither see its state nor change it without curl.
# Anchored on the WRITE the control performs, not on the key appearing in the
# file. `grep -q oauth_auto_create` still matches `oauth_auto_create_REMOVED`,
# and a mention in a comment is not a control — the same substring trap that let
# a renamed `const RSPAMD_MILTER` keep a pin green at s270.
for key in self_registration_enabled oauth_auto_create; do
  if grep -qF "{ $key:" "$SETTINGS_TSX"; then
    ok "$key has a control that writes it in Settings.tsx"
  else
    bad "$key is a DB-only switch again — it gates account creation and no operator can see or flip it in the panel"
  fi
done

# A control must also be SAVEABLE. If the key is missing from the PUT allowlist
# the toggle renders, the operator flips it, and the request 400s "Unknown
# setting".
#
# Scoped to the ALLOWED_KEYS const. settings.rs used to hold TWO allowlists — one
# for PUT /api/settings and one for import_config — so this arm read only the
# update function's copy, because a file-wide grep was satisfied by the import
# list alone and said nothing about whether the toggle could save. s276 merged
# them into one const (the two had already drifted); settings-controls-pin-e2e.sh
# §5 now fails if a second inline list ever reappears.
#
# Comment lines are stripped before matching: the const carries prose about which
# keys it holds, and a pin that greps raw source is satisfied by the explanation
# of a key rather than the key itself.
UPDATE_FN=$(sed -n '/pub const ALLOWED_KEYS/,/^\];/p' "$SETTINGS" | grep -v '^\s*//')
if [ -z "$UPDATE_FN" ]; then
  bad "could not isolate ALLOWED_KEYS in $SETTINGS — the allowlist arms below are reading nothing"
else
  for key in self_registration_enabled oauth_auto_create; do
    if printf '%s\n' "$UPDATE_FN" | grep -cF "\"$key\"" >/dev/null; then
      ok "$key is in the PUT /api/settings write allowlist"
    else
      bad "$key is not writable through PUT /api/settings — its toggle would fail with 'Unknown setting'"
    fi
  done
fi

echo
echo "── 4. the control tells the truth about the default it reflects ──"

# The subtle one, and the reason this section exists separately from section 3.
#
# The two keys have OPPOSITE defaults: self_registration_enabled absent means
# CLOSED, oauth_auto_create absent means OPEN. So they cannot be rendered the
# same way. `settings.oauth_auto_create === "true"` would draw the toggle OFF on
# every install that has never set the row, while the server happily created
# accounts — a control that lies in the reassuring direction, which is worse
# than no control at all because it ends the operator's investigation.
if grep -qF 'settings.oauth_auto_create !== "false"' "$SETTINGS_TSX"; then
  ok "the OAuth toggle renders absent-as-ON, matching the backend's unwrap_or(true)"
else
  bad "the OAuth auto-registration toggle does not render an absent row as ON — it disagrees with the server's own default and would show OFF while signups succeed"
fi

if grep -qF 'settings.self_registration_enabled === "true"' "$SETTINGS_TSX"; then
  ok "the self-registration toggle renders absent-as-OFF, matching its unwrap_or(false)"
else
  bad "the self-registration toggle no longer renders an absent row as OFF — it disagrees with the server's own default"
fi

echo
echo "── 5. the first-admin door closes behind itself ──"

# POST /api/auth/setup creates the initial admin and must refuse once ANY user
# exists, or a stranger who finds the panel before its owner finishes installing
# owns the box. Checked by the count, before the password is hashed.
if grep -A 12 'pub async fn setup' "$AUTH" | grep -c 'SELECT COUNT(\*) FROM users' >/dev/null; then
  ok "setup counts existing users before doing anything"
else
  bad "$AUTH's setup route no longer counts users first — the initial-admin endpoint may be re-callable"
fi

if grep -A 16 'pub async fn setup' "$AUTH" | grep -c 'Setup already completed' >/dev/null; then
  ok "setup refuses once a user exists"
else
  bad "$AUTH's setup route no longer refuses after the first user — anyone reaching it could create another admin"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
