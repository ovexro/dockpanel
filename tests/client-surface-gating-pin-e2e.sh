#!/usr/bin/env bash
# client-surface-gating-pin-e2e.sh — s351
#
# Pins the six client-visible surfaces whose controls asked for something the
# server would refuse, and the census idiom that hid two of them.
#
# WHY THIS SUITE EXISTS. s349 fixed this shape for Mail (#430): eight of nine
# mount requests were administrator-only and every `.catch` swallowed the 403
# into an empty state, so a site owner was shown "Mail Server Not Installed" for
# a mail server running fine. The fix was correct and the sweep that followed it
# was not — it enumerated administrator-gated handlers by grepping `AdminUser(`,
# and TWO route modules never wrote that spelling. They bound the extractor
# without destructuring it, so a paren-anchored search returned zero on both, and
# both were left out of every count. Those two modules are exactly the ones
# behind the tabs that stayed broken for another two sessions.
#
# s437 UPDATE: those same two modules (on_call.rs, escalation_policies.rs) were
# exactly the ones grepped-verified to hold ALL FIVE remaining occurrences each
# of the undestructured spelling in `panel/backend/src/routes/` — the entire
# population. Adding tenant scoping to both required `claims.sub` in every
# handler, which meant destructuring the extractor everywhere it was bound —
# so the undestructured spelling is now retired from the codebase, not merely
# from these two files. §A below was rewritten to reflect that: A1/A2 no
# longer demonstrate the blind spot (there is nothing left to be blind to) and
# instead pin the retirement itself as a fact worth not regressing silently on.
#
# So the arm worth the most here is not any single page: it is §A, which fails
# when a THIRD spelling appears. A census keyed on one way of writing the thing
# it counts reports a confident number and cannot see what it missed.
#
# Coverage before this file: `grep -rln 'Alerts.tsx\|Certificates.tsx' tests/`
# returned ZERO across all 61 suites (control: `grep -rln 'Mail.tsx' tests/`
# returns 3). Every fix below would have shipped unpinned.
#
#   §A  THE CENSUS, and the reason the other five went unseen: the set of
#       spellings that gate a handler is closed and enumerated. A new idiom
#       reddens this suite instead of silently shrinking the next sweep.
#   §B  Alerts: three tabs whose every endpoint is administrator-only are gated
#       at the TRIGGER and at the MOUNT. Hiding a button decides what is
#       offered; only the mount decides what is requested.
#   §C  Certificates: the row actions are gated and the LIST is NOT. Over-fixing
#       this one would take a client's own certificates away from them, so the
#       negative arm is as load-bearing as the positive.
#   §D  the per-site backup picker reads the route that will answer it.
#   §E  THE SEVERED PAIR, cross-handler: the route that LISTS destinations and
#       the route that ACCEPTS one share a single predicate. A caller may not be
#       offered a choice the writer will reject, nor denied one it would take.
#   §F  Terminal's share control is gated; its clipboard sibling is not.
#   §G  Dashboard recommendations resolve their link through the nav registry.
#   §H  administrator pages refuse BEFORE they fetch.
#
# Pure source analysis: no box, no network, no build.

set -u
cd "$(dirname "$0")/.."
REPO=$(pwd)

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

ROUTES="$REPO/panel/backend/src/routes"
ALERTS="$REPO/panel/frontend/src/pages/Alerts.tsx"
CERTS="$REPO/panel/frontend/src/pages/Certificates.tsx"
BACKUPS="$REPO/panel/frontend/src/pages/Backups.tsx"
TERMINAL="$REPO/panel/frontend/src/pages/Terminal.tsx"
DASH="$REPO/panel/frontend/src/pages/Dashboard.tsx"
DESTS="$ROUTES/backup_destinations.rs"
SCHED="$ROUTES/backup_schedules.rs"
MODRS="$ROUTES/mod.rs"

for f in "$ALERTS" "$CERTS" "$BACKUPS" "$TERMINAL" "$DASH" "$DESTS" "$SCHED" "$MODRS"; do
  [ -f "$f" ] || { echo "FATAL: missing subject $f"; exit 1; }
done
[ -d "$ROUTES" ] || { echo "FATAL: missing routes dir $ROUTES"; exit 1; }

# Strip comments before measuring. Load-bearing here: every subject is commented
# with the reason for its gate, and those comments NAME the tokens the arms grep
# for — `isAdmin`, `AdminUser`, `require_admin`, `selectable`. Grepping raw
# source would match the sentence explaining the check instead of the check
# ([[feedback_source_pin_prose_trap]]). JSX `{/* ... */}` is handled first and
# non-greedily because five of the eight subjects are .tsx.
code() {
  perl -0777 -pe '
    s{\{\s*/\*.*?\*/\s*\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*///.*$}{}gm;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

has()         { grep -qE -- "$2" <<< "$1"; }
hasnt()       { ! grep -qE -- "$2" <<< "$1"; }
occurrences() { grep -oE -- "$2" <<< "$1" | wc -l; }

ALERTS_S=$(code "$ALERTS")
CERTS_S=$(code "$CERTS")
BACKUPS_S=$(code "$BACKUPS")
TERMINAL_S=$(code "$TERMINAL")
DASH_S=$(code "$DASH")
DESTS_S=$(code "$DESTS")
SCHED_S=$(code "$SCHED")

echo "client-surface-gating-pin-e2e"
echo

# ─────────────────────────────────────────────────────────────────────────────
echo "§0  the stripper actually stripped (a blinded stripper makes every arm below match its own explanation)"

# Each subject carries a comment naming a token an arm greps for. If the
# stripper is blinded these go red before any real arm can pass for the wrong
# reason. Three subjects, three comment styles: JSX braces, // in .tsx, // in .rs.
# The token must sit entirely on ONE line: these greps are line-based, so a
# phrase that wraps can never match and the arm is green against every input
# (lesson #409 — caught here by M19, which found this arm was vacuous).
if has "$ALERTS_S" 'their endpoints are administrator-only'; then
  bad "0.1 Alerts.tsx: comments survived the strip"
else
  ok  "0.1 Alerts.tsx: comments stripped"
fi

if has "$DASH_S" 'same registry the sidebar'; then
  bad "0.2 Dashboard.tsx: comments survived the strip"
else
  ok  "0.2 Dashboard.tsx: comments stripped"
fi

if has "$DESTS_S" 'cannot be forgotten in a future edit'; then
  bad "0.3 backup_destinations.rs: comments survived the strip"
else
  ok  "0.3 backup_destinations.rs: comments stripped"
fi

# And the inverse: a stripper that deleted CODE would make every ABSENCE arm
# below pass on code it merely removed (lesson #136). Assert the subjects still
# have their bulk.
ALERTS_RAW_L=$(wc -l < "$ALERTS")
ALERTS_STRIP_L=$(wc -l <<< "$ALERTS_S")
if [ "$ALERTS_STRIP_L" -gt $((ALERTS_RAW_L / 2)) ]; then
  ok  "0.4 the strip removed comments, not the file ($ALERTS_RAW_L → $ALERTS_STRIP_L lines)"
else
  bad "0.4 the strip took more than half of Alerts.tsx ($ALERTS_RAW_L → $ALERTS_STRIP_L) — subjects are truncated, absence arms are worthless"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§A  the census idiom — the mechanism that hid two of these surfaces for two sessions"

# Two route modules used to bind the admin extractor WITHOUT destructuring it.
# A search anchored on the open paren returned zero on both, which is why a
# sweep that used one reported them as carrying no administrator-gated
# handler at all. s437's tenant-scoping fix needed `claims.sub` in every
# handler of both files, which meant destructuring the extractor everywhere —
# grep-verified (s437) that these two files held the ENTIRE population of the
# undestructured spelling in $ROUTES, so it is now retired codebase-wide, not
# just from these two files. A1 now pins that retirement directly instead of
# demonstrating the blind spot it used to require.
UNNAMED_FILES=$(grep -rlE '^\s*_[a-z_]+: AdminUser,' "$ROUTES" | sort)
UNNAMED_COUNT=$(grep -rhE '^\s*_[a-z_]+: AdminUser,' "$ROUTES" | wc -l)

if [ "$UNNAMED_COUNT" -eq 0 ]; then
  ok  "A1 the undestructured spelling is retired codebase-wide (s437) — on_call.rs/escalation_policies.rs were its entire population"
else
  bad "A1 the undestructured spelling has reappeared ($UNNAMED_COUNT handler(s) in: $UNNAMED_FILES) — re-verify it stays invisible to a paren-anchored 'AdminUser(' census before trusting one again"
fi

# The historical control (A2) no longer has a subject to check blindness
# against now that A1 is 0 — a loop over an empty $UNNAMED_FILES would just
# print a hollow "ok" either way. If A1 ever goes non-zero again, restore the
# blindness check that used to live here (git log this file) before trusting
# any `AdminUser(`-anchored census again.

# THE ARM WORTH THE MOST — and it must not be enumerated the way the sweep that
# missed these two modules was. Detection is spelling-INDEPENDENT (any signature
# naming the extractor type at all); classification is enumerated. So a fourth
# idiom is DETECTED even though it is not RECOGNISED, which is exactly the case
# the paren-anchored sweep got wrong: it could only find what it already knew to
# look for, so its blind spot was invisible in its own output.
SIGS=$(awk '/^pub async fn /{c=1} c{print} /^\) ->/{c=0}' $(find "$ROUTES" -name '*.rs') 2>/dev/null)
UNCLASSIFIED=$(awk '
  /^pub async fn /   { fn=$4; sub(/\(.*/,"",fn); sig=""; c=1 }
  c                  { sig = sig $0 "\n" }
  /^\) ->/           {
    c=0
    if (sig ~ /AdminUser/ &&
        sig !~ /AdminUser\(/ &&
        sig !~ /_[a-z_]+: AdminUser/) print fn
  }
' $(find "$ROUTES" -name '*.rs') 2>/dev/null | tr '\n' ' ' | sed 's/ $//')

if [ -z "$UNCLASSIFIED" ]; then
  ok  "A3 every signature naming the admin extractor uses one of the two enumerated spellings — a census covering both covers all of them"
else
  bad "A3 a THIRD way to bind the admin extractor appeared in: $UNCLASSIFIED — every existing sweep is now blind to it, exactly as they were to the undestructured form"
fi

# A3 is only worth anything if it is looking at signatures at all.
SIG_COUNT=$(grep -c 'AdminUser' <<< "$SIGS")
if [ "$SIG_COUNT" -gt 50 ]; then
  ok  "A3b the signature scan sees $SIG_COUNT admin-extractor mentions — A3's silence means classified, not unexamined"
else
  bad "A3b the signature scan found only $SIG_COUNT admin-extractor mentions — the extraction broke and A3 passed on an empty set"
fi

# The two modules behind the Alerts tabs must still be admin-gated. If one is
# ever relaxed, §B's gating becomes wrong rather than merely redundant.
for m in on_call escalation_policies; do
  if grep -qE '(AdminUser|require_admin)' "$ROUTES/$m.rs"; then
    ok  "A4 $m.rs is administrator-gated — the tab that fronts it must stay gated too"
  else
    bad "A4 $m.rs is no longer administrator-gated — §B hides a tab a client is now allowed to use"
  fi
done

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§B  Alerts — the three administrator tabs, gated at the trigger AND at the mount"

if has "$ALERTS_S" 'useAuth'; then
  ok  "B1 Alerts.tsx reads the caller's role at all (it had no role import whatsoever)"
else
  bad "B1 Alerts.tsx has no role awareness — every tab is offered to every caller"
fi

# Triggers. Three buttons, each behind the flag.
TRIGGERS=$(occurrences "$ALERTS_S" 'isAdmin && \(')
if [ "$TRIGGERS" -ge 3 ]; then
  ok  "B2 all three administrator tab triggers are behind the role flag ($TRIGGERS guarded blocks)"
else
  bad "B2 only $TRIGGERS guarded trigger blocks — a tab a client cannot use is still being offered"
fi

# Mounts. Separate from the triggers ON PURPOSE: a presence test on the buttons
# cannot see a body that still mounts (lesson #423, and #430's whole point).
for t in runbooks 'on-call' policies; do
  if has "$ALERTS_S" "tab === \"$t\" && isAdmin"; then
    ok  "B3 the $t body will not mount without the role — hiding the trigger is not refusing the request"
  else
    bad "B3 the $t body mounts on tab state alone — any future way to set the tab re-opens the refused calls"
  fi
done

# The runbook read inside the WORKING tab. Administrator-only, and its rejection
# caches as `null`, which this component renders exactly like "none written".
if has "$ALERTS_S" 'isAdmin && runbookCache'; then
  ok  "B4 the per-alert runbook read is gated — a 403 cached as null is indistinguishable from an absent runbook"
else
  bad "B4 the runbook read fires for any caller; its refusal renders as 'no runbook exists'"
fi

# The NEGATIVE control: the Alerts tab itself is user-scoped and must stay open.
# Over-gating here would take a client's own alerts away.
if hasnt "$ALERTS_S" 'tab === "alerts" && isAdmin'; then
  ok  "B5 the Alerts tab itself is NOT gated — its endpoints are user-scoped and the owner is entitled to them"
else
  bad "B5 the Alerts tab was gated too — that removes a client's own alerts, which is a worse defect than the one being fixed"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§C  Certificates — actions gated, list deliberately not"

if has "$CERTS_S" 'useAuth'; then
  ok  "C1 Certificates.tsx reads the caller's role"
else
  bad "C1 Certificates.tsx has no role awareness — Renew and Delete are offered to callers who will be refused"
fi

if has "$CERTS_S" '\{isAdmin && <th'; then
  ok  "C2 the Actions column header is gated (an empty column is a control that half-rendered)"
else
  bad "C2 the Actions header renders for everyone — a headed column with no cells"
fi

if has "$CERTS_S" '\{isAdmin && \(' ; then
  ok  "C3 the Actions cell is gated"
else
  bad "C3 the Actions cell renders Renew/Delete to callers the API refuses"
fi

# THE NEGATIVE ARM, and it matters more than the two above. The list read is
# AuthUser on purpose and its handler says so; gating it would hand a client
# "No SSL certificates found" for certificates they own.
if hasnt "$CERTS_S" 'isAdmin.*monitors/certificates|monitors/certificates.*isAdmin'; then
  ok  "C4 the certificate LIST is not gated — the read is intentionally open and must stay open"
else
  bad "C4 the certificate list was gated — a site owner can no longer see their own certificates"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§D  the per-site backup picker asks the route that will answer it"

if has "$BACKUPS_S" 'backup-destinations/selectable'; then
  ok  "D1 the picker reads the caller-scoped destination route"
else
  bad "D1 the picker no longer reads the caller-scoped route — an administrator-only list cannot fill a site owner's dropdown"
fi

if hasnt "$BACKUPS_S" 'api\.get<BackupDestination\[\]>\("/backup-destinations"\)'; then
  ok  "D2 the picker does NOT read the administrator list route"
else
  bad "D2 the picker reads the administrator list route again — it returns 403 for exactly the callers this page serves"
fi

if has "$BACKUPS_S" 'destinationsFailed'; then
  ok  "D3 a failed destination load is remembered, so it can be told apart from an empty one"
else
  bad "D3 a failed load is indistinguishable from 'no destinations exist' — the defect nobody can report (#435)"
fi

if has "$BACKUPS_S" 'isAdmin \?' && has "$BACKUPS_S" 'backup-orchestrator\?tab=destinations'; then
  ok  "D4 the Backup Manager link is offered only to callers who can open it"
else
  bad "D4 the empty state links every caller into an administrator-only page that bounces them back"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§E  the severed pair — the reader and the writer share one predicate"

if has "$DESTS_S" 'pub const DESTINATION_CALLER_PREDICATE'; then
  ok  "E1 the predicate is declared once"
else
  bad "E1 the shared predicate is gone — reader and writer are free to drift again"
fi

# Both sides must INTERPOLATE it rather than spell the rule out. Two copies of a
# rule is how this pair came apart in the first place.
if has "$DESTS_S" 'WHERE \{DESTINATION_CALLER_PREDICATE\}'; then
  ok  "E2 the READER interpolates the shared predicate"
else
  bad "E2 the reader carries its own copy of the rule — the set a caller is offered can drift from the set the writer accepts"
fi

if has "$SCHED_S" 'DESTINATION_CALLER_PREDICATE'; then
  ok  "E3 the WRITER interpolates the shared predicate"
else
  bad "E3 the writer carries its own copy of the rule — same drift, other direction"
fi

# The reader must be reachable by the callers the writer admits. If it is ever
# tightened back to AdminUser the pair is severed again, with the writer still
# granting a permission nothing can enumerate.
SEL_SIG=$(sed -n '/pub async fn selectable/,/^) ->/p' <<< "$DESTS_S")
if has "$SEL_SIG" 'AuthUser'; then
  ok  "E4 the reader is open to every authenticated caller, as the writer already is"
else
  bad "E4 the reader is no longer AuthUser — the writer still admits site owners to a choice they cannot see"
fi

# And it must not be able to leak the credentials. The lean struct has no
# `config` field at all; an arm on the masking pass would pass the day someone
# forgets to call it.
SEL_STRUCT=$(sed -n '/pub struct SelectableDestination/,/^}/p' <<< "$DESTS_S")
if [ -n "$SEL_STRUCT" ] && hasnt "$SEL_STRUCT" 'config'; then
  ok  "E5 the caller-facing struct has no credentials field to forget to mask"
else
  bad "E5 the caller-facing struct carries config — bucket credentials are one serialization away from every site owner"
fi

if grep -qE '"/api/backup-destinations/selectable"' "$MODRS"; then
  ok  "E6 the caller-scoped route is registered"
else
  bad "E6 the caller-scoped route is not registered — the picker calls an endpoint that 404s"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§F  Terminal — the share control is gated, the clipboard one is not"

# Anchored on the SHARE control specifically. A bare search for a guarded button
# passes on the SSH Info button further down the same file, which was already
# gated — so the arm would have reported the share button safe while it was open
# (lesson #436: a mutation that survives is a claim about the arm first, and M15
# is what exposed this).
SHARE_GUARDED=$(awk '
  /\{isAdmin && <button/  { guarded=1; inbtn=1; next }
  /<button/               { guarded=0; inbtn=1; next }
  /\/terminal\/share/     { if (inbtn) { print (guarded ? "GUARDED" : "OPEN"); exit } }
' <<< "$TERMINAL_S")
if [ "$SHARE_GUARDED" = "GUARDED" ]; then
  ok  "F1 the control that posts to the share endpoint is the one behind the role flag"
elif [ "$SHARE_GUARDED" = "OPEN" ]; then
  bad "F1 the share control is offered to callers whose request calls require_admin"
else
  bad "F1 could not locate the share control at all — the arm is measuring nothing, not passing"
fi

SHARE_BODY=$(sed -n '/pub async fn share_output/,/^}/p' "$ROUTES/terminal.rs")
if has "$SHARE_BODY" 'require_admin'; then
  ok  "F2 the share endpoint is still administrator-only, so F1's gate is still correct"
else
  bad "F2 the share endpoint was opened up — F1 now hides a control the caller is allowed to use"
fi

# NEGATIVE control: Copy Output never leaves the browser and must stay open.
if hasnt "$TERMINAL_S" 'isAdmin && <button[^>]*Copy Output'; then
  ok  "F3 Copy Output is NOT gated — it makes no request and belongs to whoever opened the shell"
else
  bad "F3 Copy Output was gated — that removes a local, request-free capability for no reason"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§G  Dashboard recommendations resolve through the nav registry"

if has "$DASH_S" 'isNavVisible\(navFlagsFor\('; then
  ok  "G1 the recommendation link is resolved through the same registry the sidebar uses"
else
  bad "G1 the recommendation link is taken straight from the action map — three of its six routes are administrator-only"
fi

# The message must survive even when the link does not: a client whose own site
# has no backup is entitled to be told.
if has "$DASH_S" 'route \? \(' || has "$DASH_S" 'return route \?'; then
  ok  "G2 a row with no reachable route still renders its message"
else
  bad "G2 gating the route also dropped the recommendation — the client is no longer told about their own stale backup"
fi

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "§H  administrator pages refuse before they fetch"

# A redirect is an effect. Placed below the loaders it does not out-run them, so
# the mount spends its whole request burst on 403s first.
for p in Security Users ContainerPolicies; do
  f="$REPO/panel/frontend/src/pages/$p.tsx"
  [ -f "$f" ] || { bad "H1 $p.tsx is missing"; continue; }
  S=$(code "$f")
  GUARD_LINE=$(grep -nE 'role !== "admin"\) return <Navigate' <<< "$S" | head -1 | cut -d: -f1)
  EFFECT_LINE=$(grep -nE '^\s*useEffect\(' <<< "$S" | head -1 | cut -d: -f1)
  if [ -z "$GUARD_LINE" ]; then
    bad "H1 $p.tsx has no role guard at all"
  elif [ -z "$EFFECT_LINE" ]; then
    ok  "H1 $p.tsx guards on role and registers no effect to out-run"
  elif [ "$GUARD_LINE" -lt "$EFFECT_LINE" ]; then
    ok  "H1 $p.tsx refuses at line $GUARD_LINE, before its first effect at $EFFECT_LINE"
  else
    bad "H1 $p.tsx guards at line $GUARD_LINE but registers an effect at $EFFECT_LINE — the requests fire and 403 before the redirect lands"
  fi
done

# ─────────────────────────────────────────────────────────────────────────────
echo
printf 'PASS %d FAIL %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
