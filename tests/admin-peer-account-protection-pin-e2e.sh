#!/usr/bin/env bash
# admin-peer-account-protection-pin-e2e.sh — s440
#
# ONE PROPERTY: an administrator can act on the user directory, but not on
# ANOTHER administrator's account.
#
# The v2.176.0->v2.195.0 arc closed the "any admin can touch any OTHER
# TENANT's resource" bug class across alerts/on_call/escalation_policies/
# dashboard/drift/servers/monitors/incidents — but never applied it to the
# one resource whose compromise subsumes all the others: the user account
# itself. Every mutation in `panel/backend/src/routes/users.rs` was gated
# only on `AdminUser` (a role check, nothing else), so any two admin
# accounts on the same install were fully interchangeable:
#   - `reset_password` set ANY user's password, including another admin's
#     -> full account takeover, no code or consent from the victim.
#   - `update`'s password branch was the same hole through a second door.
#   - `remove` deleted another admin AND auto-transferred every server they
#     registered to the caller (`UPDATE servers SET user_id = <caller>`)
#     -> one DELETE call inherits a peer's entire infrastructure.
#   - `toggle_suspend` could lock out a peer admin with no recourse.
# Multiple administrators is a real, supported configuration, not a
# contrived one: `docs/guides/roles-and-ownership.md` describes a per-admin
# hardware boundary for SITES ("does not extend to a machine another
# administrator added"), and 534dc51/v2.185.0 fixed a second-admin-
# visibility bug elsewhere in this same arc.
#
# `list`/`create` and `reset_2fa` are DELIBERATELY untouched by the fix:
#   - `list` mirrors `servers.rs`'s own established idiom for an
#     `AdminUser`-only surface (the filter is dropped outright there too;
#     seeing the directory does not hand over an account).
#   - `create` mints a NEW account; there is no existing one to hijack.
#   - `reset_2fa` clears a factor but does not by itself grant sign-in (the
#     password is untouched) — a materially different class from the four
#     that DO hand over full control, and it remains the only panel path
#     back for an admin who has lost both their authenticator and their
#     recovery codes (see users.rs's own doc comment on reset_2fa).
# §F and §G below are CONTROLS that pin those exclusions as deliberate, not
# as gaps this suite failed to notice.
#
# §A  the guard function itself exists and checks the right two things.
# §B  update()          calls it, BEFORE either mutating branch.
# §C  reset_password()  calls it, BEFORE the password UPDATE.
# §D  remove()          calls it, BEFORE the delete/server-reassignment.
# §E  toggle_suspend()  calls it, BEFORE the role-flip.
# §F  reset_2fa()       does NOT call it (control, see above).
# §G  list()/create()   do NOT call it (control, see above).
# §H  each rejection message names the action it is refusing.
#
# Position arms (B2/C2/D2/E2), not just presence, because a check that
# exists in the function but runs AFTER the mutation is a check that does
# nothing — the same lesson project_dockpanel_tech_debt_p178's
# registry_login mutation drew: a text-presence arm alone cannot tell
# "guarded" from "guarded too late".

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  Admin peer-account protection — source pins (s440)"
echo "=================================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip comments before matching, or an arm can be satisfied by the prose
# that NARRATES the check rather than the check itself.
code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()  { grep -qE -- "$2" <<< "$1"; }

# A function body, bounded on ITS OWN braces. Anchored on `fn NAME(`, not the
# bare name, so a rename to a prefixed/suffixed sibling cannot satisfy it.
fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn "(") && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

# Byte offset of the FIRST match of a literal substring within a body, or
# -1 if absent. Used to prove a guard runs BEFORE a mutation, not merely
# that both strings exist somewhere in the same function.
offset() {
  local body="$1" needle="$2"
  local head="${body%%"$needle"*}"
  if [ "$head" = "$body" ]; then
    echo -1
  else
    echo "${#head}"
  fi
}

before() {
  local body="$1" first="$2" second="$3"
  local o1 o2
  o1=$(offset "$body" "$first")
  o2=$(offset "$body" "$second")
  [ "$o1" != -1 ] && [ "$o2" != -1 ] && [ "$o1" -lt "$o2" ]
}

USERS=panel/backend/src/routes/users.rs
USERS_C=$(code "$USERS")

UPDATE_BODY=$(fnbody "$USERS_C" "update")
RESET_PW_BODY=$(fnbody "$USERS_C" "reset_password")
REMOVE_BODY=$(fnbody "$USERS_C" "remove")
TOGGLE_BODY=$(fnbody "$USERS_C" "toggle_suspend")
RESET_2FA_BODY=$(fnbody "$USERS_C" "reset_2fa")
LIST_BODY=$(fnbody "$USERS_C" "list")
CREATE_BODY=$(fnbody "$USERS_C" "create")
HELPER_BODY=$(fnbody "$USERS_C" "is_other_admin")

# ── §A the guard itself ─────────────────────────────────────────────────────
echo "── §A is_other_admin exists and checks role + identity ──"

if [ -n "$HELPER_BODY" ]; then
  ok "A1 is_other_admin exists"
else
  bad "A1 is_other_admin is missing — every arm below is measuring nothing"
fi

if has "$HELPER_BODY" 'role\s*==\s*"admin"'; then
  ok "A2 the guard checks the target's role against \"admin\""
else
  bad "A2 the guard does not check for role == \"admin\" — it cannot be the peer-admin gate"
fi

if has "$HELPER_BODY" '!='; then
  ok "A3 the guard excludes the caller's OWN id (an inequality check is present)"
else
  bad "A3 the guard has no inequality check — it would also block an admin acting on themselves"
fi

# ── §B update() ──────────────────────────────────────────────────────────────
echo "── §B update() is gated before either mutating branch ──"

if has "$UPDATE_BODY" 'is_other_admin'; then
  ok "B1 update() calls is_other_admin"
else
  bad "B1 update() never calls is_other_admin — the role AND password branches are both still open"
fi

if before "$UPDATE_BODY" "is_other_admin" "SET role"; then
  ok "B2 the guard runs before the role-change UPDATE"
else
  bad "B2 the guard does not precede the role-change UPDATE — present-but-too-late is the same as absent"
fi

if before "$UPDATE_BODY" "is_other_admin" "SET password_hash"; then
  ok "B3 the guard runs before the password-change UPDATE"
else
  bad "B3 the guard does not precede the password-change UPDATE — a second, ungated door to the same takeover"
fi

# ── §C reset_password() ─────────────────────────────────────────────────────
echo "── §C reset_password() is gated before the password UPDATE ──"

if has "$RESET_PW_BODY" 'is_other_admin'; then
  ok "C1 reset_password() calls is_other_admin"
else
  bad "C1 reset_password() never calls is_other_admin — one admin can still take over another's account"
fi

if before "$RESET_PW_BODY" "is_other_admin" "SET password_hash"; then
  ok "C2 the guard runs before the password UPDATE"
else
  bad "C2 the guard does not precede the password UPDATE — present-but-too-late is the same as absent"
fi

# ── §D remove() ──────────────────────────────────────────────────────────────
echo "── §D remove() is gated before the delete + server reassignment ──"

if has "$REMOVE_BODY" 'is_other_admin'; then
  ok "D1 remove() calls is_other_admin"
else
  bad "D1 remove() never calls is_other_admin — deleting a peer admin still transfers their fleet to the caller"
fi

if before "$REMOVE_BODY" "is_other_admin" "SET user_id = \$1"; then
  ok "D2 the guard runs before the server-reassignment UPDATE"
else
  bad "D2 the guard does not precede the server reassignment — present-but-too-late is the same as absent"
fi

if before "$REMOVE_BODY" "is_other_admin" "DELETE FROM users"; then
  ok "D3 the guard runs before the DELETE"
else
  bad "D3 the guard does not precede the DELETE — present-but-too-late is the same as absent"
fi

# ── §E toggle_suspend() ──────────────────────────────────────────────────────
echo "── §E toggle_suspend() is gated before the role flip ──"

if has "$TOGGLE_BODY" 'is_other_admin'; then
  ok "E1 toggle_suspend() calls is_other_admin"
else
  bad "E1 toggle_suspend() never calls is_other_admin — a peer admin can still be locked out unilaterally"
fi

if before "$TOGGLE_BODY" "is_other_admin" "suspend_account"; then
  ok "E2 the guard runs before the suspend/unsuspend helpers"
else
  bad "E2 the guard does not precede the suspend/unsuspend call — present-but-too-late is the same as absent"
fi

# ── §F reset_2fa() — deliberately UNGUARDED (control) ────────────────────────
echo "── §F reset_2fa() deliberately does not call the guard ──"

if ! has "$RESET_2FA_BODY" 'is_other_admin'; then
  ok "F1 reset_2fa() does not call is_other_admin — deliberate, it does not grant sign-in by itself"
else
  bad "F1 reset_2fa() now calls is_other_admin — the documented recovery path for a locked-out peer admin is gone; if this was intentional, update this suite AND the doc comment together"
fi

# ── §G list()/create() — deliberately UNGUARDED (control) ───────────────────
echo "── §G list()/create() deliberately do not call the guard ──"

if ! has "$LIST_BODY" 'is_other_admin'; then
  ok "G1 list() does not call is_other_admin — seeing the directory does not hand over an account"
else
  bad "G1 list() now calls is_other_admin — an admin would no longer see the full user directory, a behaviour change beyond this fix's scope"
fi

if ! has "$CREATE_BODY" 'is_other_admin'; then
  ok "G2 create() does not call is_other_admin — there is no existing account to hijack when minting a new one"
else
  bad "G2 create() now calls is_other_admin — there is no target row yet for the guard to inspect"
fi

# ── §H rejection messages name the action they refuse ────────────────────────
echo "── §H each message is specific to its own handler ──"

if has "$UPDATE_BODY" "modify another administrator"; then
  ok "H1 update()'s rejection names \"modify\""
else
  bad "H1 update()'s rejection message is missing or generic"
fi

if has "$RESET_PW_BODY" "reset another administrator's password"; then
  ok "H2 reset_password()'s rejection names \"password\""
else
  bad "H2 reset_password()'s rejection message is missing or generic"
fi

if has "$REMOVE_BODY" "delete another administrator"; then
  ok "H3 remove()'s rejection names \"delete\""
else
  bad "H3 remove()'s rejection message is missing or generic"
fi

if has "$TOGGLE_BODY" "suspend another administrator"; then
  ok "H4 toggle_suspend()'s rejection names \"suspend\""
else
  bad "H4 toggle_suspend()'s rejection message is missing or generic"
fi

echo
echo "=================================================="
echo "  PASS=$PASS FAIL=$FAIL"
echo "=================================================="

[ "$FAIL" -eq 0 ]
