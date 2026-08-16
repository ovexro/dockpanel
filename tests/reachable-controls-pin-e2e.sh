#!/usr/bin/env bash
# Regression pins for the s368 ship — verdicts the panel can support, and controls
# an operator can actually reach.
#
# Two shapes, one release.
#
# THE VERDICT IT CANNOT SUPPORT:
#   * The mail Reputation card rendered a green "Clean" when all eight blacklist
#     lookups had FAILED. A DNSxL answers "listed" with an A record and "not listed"
#     with NXDOMAIN, so a bare success test scores a dead resolver, a timeout and a
#     retired zone as a clean bill of health — and the per-zone detail panel was
#     gated on the verdict, so it hid exactly when the verdict was worthless.
#   * WordPress bulk update announced "Updated N site(s) successfully" where N was
#     the number of ticked checkboxes. The handler answers 200 carrying a per-site
#     verdict even when every entry failed; the client never bound it.
#   * WordPress hardening announced success over a run in which every requested fix
#     reported that it had not been applied.
#   * Deleting a maintenance window answered "deleted" unconditionally, discarding
#     the statement's result. A window that survives keeps EVERY monitor belonging
#     to that account switched off, so the reassuring answer is also the expensive
#     one.
#
# THE CONTROL NOBODY COULD REACH — complete, routed, ownership-scoped handlers with
# no caller in the SPA or the CLI, which is a capability the project paid for and
# never shipped:
#   * attaching an escalation policy to an alert rule — while the Alerts page told
#     the operator, in its own words, to go and do it;
#   * listing and revoking a terminal share, which is an UNAUTHENTICATED public URL
#     carrying root scrollback;
#   * rotating an extension's webhook signing secret;
#   * linking a secrets vault to a site, without which every auto-inject tick on
#     that page is inert.
#
# §D is a CLASS arm: it enumerates the endpoints this release connected and asserts
# each has a caller, so a future refactor that severs one again is caught here.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()  { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }
ge()  { [ "$2" -ge "$3" ] 2>/dev/null && ok "$1" || bad "$1" "expected at least $3, got '$2'"; }

# ugrep's --ignore-files shim honours .gitignore, so every count below uses the
# real binary explicitly (dockpanel-ops-p2, s357).
G=/usr/bin/grep

MAIL=panel/backend/src/routes/mail.rs
MAILUI=panel/frontend/src/pages/Mail.tsx
WPUI=panel/frontend/src/pages/WordPressToolkit.tsx
MONITORS=panel/backend/src/routes/monitors.rs
SETTINGS=panel/frontend/src/pages/Settings.tsx
TERMUI=panel/frontend/src/pages/Terminal.tsx
EXTUI=panel/frontend/src/pages/Extensions.tsx
VAULTUI=panel/frontend/src/pages/SecretsManager.tsx
SECRETS=panel/backend/src/routes/secrets.rs
DBUI=panel/frontend/src/pages/Databases.tsx

for f in "$MAIL" "$MAILUI" "$WPUI" "$MONITORS" "$SETTINGS" "$TERMUI" "$EXTUI" "$VAULTUI" "$SECRETS" "$DBUI"; do
  [ -f "$f" ] || { bad "SETUP" "$f missing"; exit 1; }
done

# An arm that measures an empty subject prints green for every absence below, so
# each subject is asserted before it is measured (lesson #143).
for pair in "$MAIL:1500" "$MAILUI:800" "$WPUI:300" "$SETTINGS:2500" "$TERMUI:800"; do
  f=${pair%:*}; min=${pair##*:}
  n=$($G -c '' "$f")
  [ "$n" -gt "$min" ] && ok "A0 subject extracted — $f is $n lines" \
    || bad "A0 subject extracted" "$f is only $n lines — arms over it examined nothing"
done

echo "── B. A blacklist verdict is only as good as the zones that answered ──"

# B1: the RFC 5782 control probe. Every DNSxL must list 127.0.0.2, so a query for
# it has a KNOWN correct answer — that is what separates "asked and told no" from
# "could not ask". Keyed on the reversed literal, which cannot appear in prose by
# accident.
eq "B1 each zone is probed with an answer known in advance" \
   "$($G -c '2\.0\.0\.127\.' "$MAIL")" "1"

# B2: the verdict is not computed from the listing count alone. ABSENCE arm, so it
# needs a positive control (B2-control) proving the file is really being read.
eq "B2 the verdict is not decided by the listing count alone" \
   "$($G -c '"clean": listed_count == 0' "$MAIL")" "0"

eq "B2-control the handler still reports a listing count" \
   "$($G -c 'let listed_count' "$MAIL")" "1"

# B3: a zone that did not answer is carried as unknown rather than folded into
# either verdict.
ge "B3 the response distinguishes a zone that could not be reached" \
   "$($G -c '"checked": checked' "$MAIL")" "1"

# B4: the load-bearing line — "clean" requires that EVERY zone actually answered,
# not merely that none of them said yes. Keyed on that comparison because it is
# unique to this handler; both `"unknown"` and `"status": status` also occur
# elsewhere in this file for unrelated reasons, and an arm on either would have been
# measuring that other code instead of this decision.
eq "B4 a clean verdict requires every zone to have answered" \
   "$($G -c 'checked_count == results.len()' "$MAIL")" "1"

# B5: the card must render that third state rather than painting it as one of the
# two it already had. THREE independent sites decide it — the status dot, the text
# colour and the wording — so this counts them rather than asking whether any one
# survives. A `-ge 1` arm here passed its own mutation test: removing one site left
# the other two matching (lesson #543 — the unit must be the thing that can go
# missing).
eq "B5 all three verdict renderings read the three-state status" \
   "$($G -c 'status === "clean"' "$MAILUI")" "3"

# B6: the per-zone detail must no longer be hidden by the verdict it explains.
ge "B6 an unreachable zone is marked in the detail list" \
   "$($G -c 'no answer' "$MAILUI")" "1"

echo "── C. A per-item result is read, not replaced by the request ─────────"

# C1: the bulk banner must not be built from the size of the selection.
eq "C1 the bulk banner does not count the selection" \
   "$($G -c 'Updated ${selected.size} site(s)' "$WPUI")" "0"

eq "C1-control the bulk banner still exists for the case that earns it" \
   "$($G -c 'site(s) successfully' "$WPUI")" "1"

# C2: it counts the entries the handler marked failed.
ge "C2 the bulk banner reads the per-site verdict" \
   "$($G -c 'filter((r) => !r.ok)' "$WPUI")" "1"

# C3: hardening must not announce success without reading the per-fix verdict.
eq "C3 hardening does not announce a blanket success" \
   "$($G -c 'Hardening applied successfully' "$WPUI")" "0"

ge "C4 hardening reads the per-fix verdict" \
   "$($G -c 'filter((r) => !r.applied)' "$WPUI")" "1"

echo "── D. Every control this release connected still has a caller ────────"

# A CLASS arm. Each row is `<label>|<file>|<pattern>` — the endpoint an operator
# reaches, and the file that must reach it. A refactor that severs one of these
# again turns this red by name rather than silently restoring the defect.
#
# Iterated as a bash ARRAY rather than piped into `while read`: a pipeline puts the
# loop in a subshell, where ok/bad print their marks but cannot update the counters
# the summary is built from. The first draft did exactly that and reported PASS 25
# while printing 30 marks — a suite that misstates its own size, which is the
# published-assertion bookkeeping this repo pins in docs/testing.md.
D_ROWS=(
  "escalation policy attach|$SETTINGS|alert-rules/\${alertRuleId}/escalation-policy"
  "terminal share list|$TERMUI|\"/terminal/shares\""
  "terminal share revoke|$TERMUI|terminal/share/\${s.share_id}"
  "extension secret rotation|$EXTUI|/rotate-secret"
  "vault site link|$VAULTUI|site_id"
)
for row in "${D_ROWS[@]}"; do
  IFS='|' read -r label file pat <<<"$row"
  n=$($G -c -- "$pat" "$file" 2>/dev/null || echo 0)
  if [ "${n:-0}" -ge 1 ]; then
    ok "D1 $label is reachable from ${file##*/}"
  else
    bad "D1 $label is reachable from ${file##*/}" "no caller found"
  fi
done

# D2, the positive control this class arm needs: the same method must find a caller
# that was never severed. Without it, a broken pattern shape would report five
# healthy rows as five healthy rows for the wrong reason.
ge "D2-control the same method finds a long-standing caller" \
   "$($G -c '/terminal/share"' "$TERMUI")" "1"

# D3: linking a vault to a site is an authorization decision, because the deploy
# injector matches vaults to sites on the link ALONE. The write side must verify
# the caller may reach that site.
ge "D3 the vault site link is ownership-checked on the write side" \
   "$($G -c 'assert_site_reachable' "$SECRETS")" "3"

echo "── E. A delete that answers 'deleted' actually deleted something ──────"

# E1: the statement's result reaches the caller instead of being discarded.
ge "E1 the maintenance-window delete checks its own result" \
   "$($G -c 'delete maintenance window' "$MONITORS")" "1"

# E2: and zero rows is reported as such rather than as success. Keyed on this
# statement's own binding — two OTHER handlers in the same file already test
# `result.rows_affected() == 0`, so an arm on the bare call survived its own
# mutation by matching them instead.
ge "E2 a delete that removed nothing is not reported as success" \
   "$($G -c 'deleted.rows_affected() == 0' "$MONITORS")" "1"

echo "── F. A capability the product does not have is not advertised ────────"

# F1: point-in-time recovery is withdrawn where it was claimed.
ge "F1 the withdrawn-claims register carries point-in-time recovery" \
   "$($G -c '\*\*Point-in-Time Recovery\*\* — "WAL archiving' FEATURES.md)" "1"

# F2: and the README no longer sells it.
eq "F2 the README no longer advertises point-in-time recovery" \
   "$($G -ci 'point-in-time recovery' README.md)" "0"

# F3: the toggle says what it actually stores. Keyed on the operator-facing message
# specifically — the phrase "not implemented" also appears in the button's title
# attribute, so an arm on it stayed green when the message itself was reverted.
ge "F3 the PITR control discloses that nothing is archived" \
   "$($G -c 'nothing is being archived' "$DBUI")" "1"

echo
if [ "$FAIL" -eq 0 ]; then
  printf '\033[32mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL"
else
  printf '\033[31mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL"
fi
[ "$FAIL" -eq 0 ]
