#!/usr/bin/env bash
# client-account-door-pin-e2e.sh — s325
#
# ONE PROPERTY: a person can reach their own account, whatever their role.
#
#   §A  THE DOOR EXISTS AND IS NOT ADMIN-ONLY.
#       Password change, 2FA enrolment, passkeys, sessions and API keys all lived
#       on `/settings`, whose nav row is `adminOnly`, so `user`, `client` and
#       `reseller` accounts had no entrance to any of them. Every endpoint behind
#       them was already `AuthUser` and already scoped to the caller's own id —
#       the capability shipped long ago and was simply unreachable.
#
#   §B  THE DOOR OPENS ON ALL SIX SELF-SERVICE SURFACES.
#
#   §C  ONE IMPLEMENTATION, NOT A COPY.
#       The Settings "Account" tab and the new page render the SAME components.
#       A duplicate would drift the first time either side was touched.
#
#   §D  THE 2FA BANNER LEADS SOMEWHERE THE WARNED ROLE CAN GO.
#       All four layouts told a non-admin "Two-factor authentication is required"
#       and linked them to `/settings` — the one page their nav does not carry.
#
#   §E  A LOST AUTHENTICATOR IS RECOVERABLE.
#       `twofa_disable` accepted ONLY `check_current`, so the recovery codes let
#       you log IN and then nothing could turn 2FA off or enrol a new device, and
#       there is no admin-side 2FA reset. The frontend made it unreachable twice
#       over by stripping non-digits and demanding exactly six.
#
#   §F  A CONTROL A ROLE CANNOT USE IS NOT SHOWN TO IT.
#       Two rows inside the account tab asserted "admin only" in their own text
#       with no role check behind it, and Notifications — a page every role sees —
#       linked everyone to `/settings`.
#
#   §G  CONTEXT — arms that must be green at BOTH tags, so a harness measuring
#       nothing cannot read as a pass. Each owes a mutation (s324 #292).
#
# EVERY ARM IN §A–§F WAS RUN AGAINST v2.82.0 AND REQUIRED TO BE RED.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=============================================="
echo "  client account door — source pins (s325)"
echo "=============================================="
echo

PASS=0; FAIL=0; SKIP=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip comments before matching, or every arm can be satisfied by the prose that
# NARRATES it — and this file's own header spells each defect, as do the comments
# this ship added to the very files it pins. (s294 stripper: a block comment is
# recognised only where one is actually written.)
code() {
  # A subject this ship CREATES does not exist at the previous tag. Returning
  # empty (rather than a perl error) is what lets each arm below be demonstrated
  # RED at v2.82.0 individually, instead of the suite aborting on the first
  # missing file and proving only that it aborts.
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
    s{\{/\*.*?\*/\}}{}gs;
  ' "$1"
}

has() { grep -qE -- "$2" <<< "$1"; }
flat() { tr '\n' ' ' <<< "$1" | tr -s ' '; }

# A function body, bounded on ITS OWN braces (s323).
fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn) && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

# A JSX conditional block: from the line matching $2 to the `)}` that closes it.
# NOT `grep -A n` — a fixed window is the p40 trap and would read the next block.
# ⚠ Anchor on the RENDER CONDITION (`twoFaEnforced && !`), never the bare name:
# every one of these layouts also spells `twoFaEnforced` in its `useLayoutState`
# destructuring ~50 lines earlier, so a bare-name anchor opened the block at the
# import list and ran to an unrelated `)}`. That is s324 #290 in a JSX costume —
# the first draft of this arm went red on a tree that was already correct.
jsxblock() {
  awk -v pat="$2" '
    index($0, pat) { started=1 }
    started { print; if ($0 ~ /^[[:space:]]*\)\}[[:space:]]*$/) exit }
  ' <<< "$1"
}

# The ONE line declaring a nav row for a path. A row is the member; the file is
# not. Comparing file-wide counts is what let a mutation live at s324 (#292).
navrow() { grep -F -- "to: \"$1\"" <<< "$2"; }

NAV=panel/frontend/src/data/navItems.ts
MAIN=panel/frontend/src/main.tsx
ACCOUNT=panel/frontend/src/pages/Account.tsx
CARDS=panel/frontend/src/components/AccountSecurity.tsx
SETTINGS=panel/frontend/src/pages/Settings.tsx
NOTIF=panel/frontend/src/pages/Notifications.tsx
PALETTE=panel/frontend/src/components/CommandPalette.tsx
AUTH=panel/backend/src/routes/auth.rs

# Subjects that exist at BOTH tags. If one of these is gone the suite is pointed
# at something that is not this repo, and every arm below would be vacuous.
# `$ACCOUNT` and `$CARDS` are deliberately NOT in this list — they are what this
# ship adds, and their absence must red an arm rather than abort the run.
for f in "$NAV" "$MAIN" "$SETTINGS" "$NOTIF" "$PALETTE" "$AUTH"; do
  [ -f "$f" ] || { echo "  MISSING SUBJECT: $f"; exit 1; }
done

NAV_C=$(code "$NAV")
MAIN_C=$(code "$MAIN")
ACCOUNT_C=$(code "$ACCOUNT")
CARDS_C=$(code "$CARDS")
SETTINGS_C=$(code "$SETTINGS")
NOTIF_C=$(code "$NOTIF")
PALETTE_C=$(code "$PALETTE")
AUTH_C=$(code "$AUTH")

# ── §A the door exists and is not admin-only ─────────────────────────────────
echo "── §A the door exists, and every role can see it ──"

ROW=$(navrow /account "$NAV_C")
if [ -n "$ROW" ]; then
  ok "A1 navItems declares a /account row"
else
  bad "A1 no /account nav row — the page is unreachable from the sidebar"
fi

# Read the ROW's own text, not the file's. The file is full of `adminOnly`.
if [ -n "$ROW" ] && ! has "$ROW" 'adminOnly' && ! has "$ROW" 'resellerVisible'; then
  ok "A2 the /account row carries neither adminOnly nor resellerVisible"
else
  bad "A2 the /account row is restricted — isNavVisible hides it from the roles it exists for"
fi

# The control: /settings must STILL be adminOnly, or A2 proves nothing about
# restriction and the suite is measuring a tree where nothing is restricted.
if has "$(navrow /settings "$NAV_C")" 'adminOnly'; then
  ok "A3 /settings is still adminOnly (so A2 is a real distinction)"
else
  bad "A3 /settings is no longer adminOnly — A2 no longer distinguishes anything"
fi

if has "$(flat "$MAIN_C")" 'path="/account" element=\{<Account'; then
  ok "A4 main.tsx routes /account to the Account page"
else
  bad "A4 no /account route — the nav row leads to the catch-all redirect"
fi

if has "$(flat "$PALETTE_C")" 'path: "/account"'; then
  ok "A5 the command palette offers /account"
else
  bad "A5 Ctrl+K cannot reach the account page"
fi

# ── §B the door opens on all six surfaces ────────────────────────────────────
echo
echo "── §B the page renders every self-service card ──"

for card in TwoFactorCard PasskeysCard ChangePasswordCard SessionsCard ApiKeysCard ExportMyDataCard; do
  rendered=$(has "$(flat "$ACCOUNT_C")" "<$card */>" && echo yes || echo no)
  exported=$(has "$CARDS_C" "^export function $card\(" && echo yes || echo no)
  if [ "$rendered" = yes ] && [ "$exported" = yes ]; then
    ok "B-$card is exported and rendered on /account"
  else
    bad "B-$card missing (exported=$exported rendered=$rendered)"
  fi
done

# ── §C one implementation, not a copy ────────────────────────────────────────
echo
echo "── §C the Settings tab and the page share ONE implementation ──"

if has "$(flat "$SETTINGS_C")" 'from "../components/AccountSecurity"'; then
  ok "C1 Settings imports the shared cards"
else
  bad "C1 Settings does not import the cards — the account tab is a second copy"
fi

# Per-ENDPOINT, not two file totals. Each self-service call must live in the
# shared component and NOT be duplicated back into Settings.tsx.
for ep in '/auth/2fa/setup' '/auth/2fa/disable' '/auth/passkey/register/begin' '/auth/change-password' '/auth/sessions' '/api-keys'; do
  in_cards=$(grep -cF -- "\"$ep" <<< "$CARDS_C")
  in_settings=$(grep -cF -- "\"$ep" <<< "$SETTINGS_C")
  if [ "$in_cards" -ge 1 ] && [ "$in_settings" -eq 0 ]; then
    ok "C-$ep is called once, from the shared card"
  else
    bad "C-$ep cards=$in_cards settings=$in_settings — duplicated or lost"
  fi
done

# ── §D the banner leads somewhere the warned role can go ─────────────────────
echo
echo "── §D the 2FA banner links to a page a non-admin can open ──"

for L in CommandLayout GlassLayout AtlasLayout NexusLayout; do
  BLOCK=$(jsxblock "$(code "panel/frontend/src/components/$L.tsx")" 'twoFaEnforced && !')
  if [ -z "$BLOCK" ]; then
    bad "D-$L has no twoFaEnforced block — the banner is gone, not fixed"
  elif has "$BLOCK" '/account' && ! has "$BLOCK" '/settings'; then
    ok "D-$L sends 'Set Up 2FA' to /account"
  else
    bad "D-$L still points the warned role at a page its nav does not carry"
  fi
done

# ── §E a lost authenticator is recoverable ───────────────────────────────────
echo
echo "── §E the recovery codes can actually turn 2FA off ──"

# `try_recovery_code` is ALSO called by the login path, so a file-wide grep
# passes on a tree where `twofa_disable` never learned about it. Scope to the
# handler's OWN body — the s324 #292 lesson, applied before it could bite.
DISABLE_BODY=$(fnbody "$AUTH_C" "twofa_disable")
if has "$DISABLE_BODY" 'try_recovery_code'; then
  ok "E1 twofa_disable accepts a recovery code"
else
  bad "E1 twofa_disable takes only a live TOTP code — a lost device is a dead end"
fi

# The control for E1: the login path must still have its own call, or `fnbody`
# is simply matching everything and E1 is vacuous.
if has "$(fnbody "$AUTH_C" "twofa_verify")" 'try_recovery_code'; then
  ok "E2 twofa_verify still accepts one too (fnbody is discriminating)"
else
  bad "E2 twofa_verify lost its recovery branch — or fnbody is broken"
fi

# The UI half: the disable field must admit an 8-character hex recovery code.
DISABLE_FIELD=$(flat "$(grep -A3 'disableCode' <<< "$CARDS_C")")
if has "$DISABLE_FIELD" 'slice\(0, 8\)' && ! has "$DISABLE_FIELD" 'replace\(/.D/g'; then
  ok "E3 the disable field admits an 8-char recovery code"
else
  bad "E3 the disable field still strips non-digits or caps at 6 — the code cannot be typed"
fi

# The recovery codes must render where the user can still SEE them. At v2.82.0
# the block sat INSIDE the `twoFaSetup ?` branch while the enable handler ran
# `setTwoFaSetup(null)` in the same breath — so the branch went false and the
# codes were never drawn, under a toast reading "2FA enabled! Save your recovery
# codes." Nobody enrolling ever received the one thing that gets them back in.
# Structural, not textual: assert the block is NOT inside the enabled/setup
# ternary. Indentation would be a weaker test; this reads the region.
TERNARY=$(awk '
  index($0, "{enabled ? (") { s=1 }
  s { print; if ($0 ~ /^[[:space:]]{8}\)\}[[:space:]]*$/) exit }
' <<< "$CARDS_C")
if has "$CARDS_C" 'recoveryCodes.length > 0' && ! has "$TERNARY" 'recoveryCodes.length > 0'; then
  ok "E4 the recovery codes render outside the setup branch — enrolling shows them"
else
  bad "E4 the recovery codes are nested in a branch that is false by the time they exist"
fi

# ── §F controls a role cannot use are not shown to it ────────────────────────
echo
echo "── §F no control is offered to a role that cannot use it ──"

# Each block is checked in ITS OWN slice. A file-wide count of `role === "admin"`
# would be satisfied by the three siblings that were always gated.
# ⚠ The marker must be unique to the RENDERED block. Keying "Panel-Wide Sessions"
# on `revoke_sessions` matched the `case "revoke_sessions":` arm in
# `executeConfirm` ~1100 lines earlier and read a slice with no gate in it — the
# arm went red over a block that is correctly gated. Each block's own heading is
# the one string that names it exactly once.
for pair in '2FA Enforcement:>2FA Enforcement<' 'Panel-Wide Sessions:>Panel-Wide Sessions<'; do
  label=${pair%%:*}; marker=${pair##*:}
  SLICE=$(awk -v m="$marker" 'index($0,m) && !found {found=NR} {l[NR]=$0} END{if(found) for(i=(found>8?found-8:1);i<=found;i++) print l[i]}' <<< "$SETTINGS_C")
  if has "$SLICE" 'user\?\.role === "admin"'; then
    ok "F-$label is behind a role check"
  else
    bad "F-$label renders for every account that reaches /settings"
  fi
done

NOTIF_LINK=$(awk 'index($0,"Configure alert channels") && !found {found=NR} {l[NR]=$0} END{if(found) for(i=(found>6?found-6:1);i<=found;i++) print l[i]}' <<< "$NOTIF_C")
if has "$NOTIF_LINK" 'user\?\.role === "admin"'; then
  ok "F-Notifications no longer sends a client to a page that refuses them"
else
  bad "F-Notifications still offers every role a link to /settings"
fi

# ── §G controls ──────────────────────────────────────────────────────────────
echo
echo "── §G context (green at both tags — each owes a mutation) ──"

if [ "$(grep -c 'to: "/' <<< "$NAV_C")" -ge 20 ]; then
  ok "G1 navItems still parses as a registry ($(grep -c 'to: "/' <<< "$NAV_C") rows) — §A is non-vacuous"
else
  bad "G1 navItems yielded almost no rows — the reader is broken"
fi

if [ "${#CARDS_C}" -gt 8000 ]; then
  ok "G2 the shared card module read $((${#CARDS_C}/1024))KB — §B and §C are non-vacuous"
else
  bad "G2 AccountSecurity.tsx is implausibly small (${#CARDS_C} bytes)"
fi

# Anchored on the name's END: `pub async fn twofa_disable` is a PREFIX of
# `twofa_disable_renamed`, so the unanchored form survived a mutation that
# renamed the handler out from under §E entirely.
if has "$AUTH_C" 'pub async fn twofa_disable\('; then
  ok "G3 twofa_disable still exists (control for §E)"
else
  bad "G3 twofa_disable is gone — §E is measuring nothing"
fi

echo
echo "----------------------------------------------"
printf '  PASS %d  FAIL %d  SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
echo "----------------------------------------------"
echo

[ "$FAIL" -eq 0 ] || exit 1
exit 0
