#!/usr/bin/env bash
# alert-controls-pin-e2e.sh — the controls an operator uses to shape their paging
#
#   §A  the suppression vocabulary is SINGLE-SOURCED across the two trees, and
#       every alert type that reaches the fan-out is in it. The grid was a
#       hand-written ten while the panel had grown to twenty producers, so half
#       the types that page an operator — every certificate-renewal failure
#       among them — had no per-type control at all, and the one labelled "SSL"
#       governed a different alert from the one paging them.
#   §B  a suppression token that names nothing is REFUSED on the live path and
#       DROPPED on the restore path. Stored unchecked it echoed back on every
#       read as though it had worked.
#   §C  an exhausted escalation chain does NOT go permanently silent. It used
#       to, which made attaching a policy strictly worse than attaching none.
#   §D  the operator-facing copy says what the code does, on all three surfaces.
#
# Pure source analysis: no box, no network, no build.
#
# MUTATION-TESTED at HEAD — see the run recorded in the ledger. Every arm is
# scoped to a FUNCTION BODY or a bounded block, never a whole file: a
# whole-file `has` is satisfied by a sibling elsewhere in the same file.
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on
# its first match and the arm goes red on correct code. Every arm uses a
# here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

NOTIF=panel/backend/src/services/notifications.rs
ENGINE=panel/backend/src/services/alert_engine.rs
HEALER=panel/backend/src/services/auto_healer.rs
SCANNER=panel/backend/src/services/security_scanner.rs
RULES=panel/backend/src/routes/alerts.rs
SETTINGS_RS=panel/backend/src/routes/settings.rs
SETTINGS_TSX=panel/frontend/src/pages/Settings.tsx
ALERTS_TSX=panel/frontend/src/pages/Alerts.tsx
GUIDE=docs/guides/monitoring.md

for f in "$NOTIF" "$ENGINE" "$HEALER" "$SCANNER" "$RULES" "$SETTINGS_RS" \
         "$SETTINGS_TSX" "$ALERTS_TSX" "$GUIDE"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Every call to $2 in file $1, one per line, arguments PAREN-BALANCED and
# whitespace-flattened. Immune to rustfmt reflowing a call across lines.
calls() {
  perl -0777 -ne '
    while (/\b\Q'"$2"'\E\s*(\((?:[^()]++|(?1))*\))/gs) {
      my $c = $1; $c =~ s/\s+/ /g; print "$c\n";
    }
  ' "$1"
}

# The tokens of the backend vocabulary, one per line. Bounded by the const's
# own block so a sibling array elsewhere in the file cannot satisfy it.
BACK=$(sed -n '/^pub const SUPPRESSIBLE_ALERT_TYPES: &\[&str\] = &\[/,/^\];/p' "$NOTIF" \
       | grep -oE '^\s*"[a-z_]+",' | grep -oE '[a-z_]+' | sort -u)
NBACK=$(grep -c . <<< "$BACK")

# The tokens of the frontend grid, same treatment.
FRONT=$(sed -n '/^const SUPPRESSIBLE_ALERT_TYPES: { key: string; label: string }\[\] = \[/,/^\];/p' "$SETTINGS_TSX" \
        | grep -oE 'key: "[a-z_]+"' | grep -oE '"[a-z_]+"' | tr -d '"' | sort -u)
NFRONT=$(grep -c . <<< "$FRONT")

echo "== §A  one vocabulary, and it covers everything that pages =="

# A1: ENUMERATION SOUNDNESS FIRST (#143). An empty extraction would make every
# comparison below vacuously green — both lists equal, both empty.
if [ "$NBACK" -ge 15 ] && [ "$NFRONT" -ge 15 ]; then
  ok "A1 enumeration is sound — backend lists $NBACK types, the grid lists $NFRONT"
else
  bad "A1 enumeration is sound — backend $NBACK, grid $NFRONT (expected >= 15 each; a low count means the extractor broke, not that the list shrank)"
fi

# A2: the two trees agree EXACTLY. They are separate files with separate
# spellings, so this is a real comparison and not a restatement of one of them.
ONLY_BACK=$(comm -23 <(printf '%s\n' "$BACK") <(printf '%s\n' "$FRONT") | grep -c .)
ONLY_FRONT=$(comm -13 <(printf '%s\n' "$BACK") <(printf '%s\n' "$FRONT") | grep -c .)
if [ "$ONLY_BACK" -eq 0 ] && [ "$ONLY_FRONT" -eq 0 ]; then
  ok "A2 the backend vocabulary and the Settings grid name exactly the same types"
else
  bad "A2 backend and grid disagree — $ONLY_BACK backend-only, $ONLY_FRONT grid-only"
  comm -23 <(printf '%s\n' "$BACK") <(printf '%s\n' "$FRONT") | while read -r t; do
    [ -n "$t" ] && printf '        backend-only: %s\n' "$t"; done
  comm -13 <(printf '%s\n' "$BACK") <(printf '%s\n' "$FRONT") | while read -r t; do
    [ -n "$t" ] && printf '        grid-only: %s\n' "$t"; done
fi

# A3: the grid must RENDER from the constant. An inline array literal at the
# JSX site is how the two halves drifted apart the first time, and it is
# invisible to A2 — which would go on comparing a constant nothing renders.
GRIDBLOCK=$(sed -n '/Suppress External Notifications/,/GPU Alert Thresholds/p' "$SETTINGS_TSX")
if grep -qE 'SUPPRESSIBLE_ALERT_TYPES\.map\(' <<< "$GRIDBLOCK" \
   && [ "$(grep -cE 'key: "[a-z_]+", label:' <<< "$GRIDBLOCK")" -eq 0 ]; then
  ok "A3 the grid renders from the shared constant, with no inline list at the JSX site"
else
  ok_inline=$(grep -cE 'key: "[a-z_]+", label:' <<< "$GRIDBLOCK")
  bad "A3 the grid renders from the shared constant — $ok_inline inline entr(ies) found at the JSX site"
fi

# A4: THE REAL INVARIANT. Every alert type handed to the fire_alert family must
# be suppressible. This reads the PRODUCERS, not either list, so it cannot be
# satisfied by the lists agreeing with each other.
PRODUCED=$( { calls "$ENGINE" fire_alert_with_retry
              calls "$ENGINE" 'notifications::try_fire_alert'
              calls "$HEALER" 'notifications::fire_alert_deduped'
              calls "$SCANNER" 'notifications::fire_alert_deduped'
              calls "$SCANNER" 'notifications::fire_alert'; } 2>/dev/null \
          | grep -oE '"[a-z][a-z_]{3,}"' | tr -d '"' \
          | grep -vE '^(critical|warning|info|all_channels)$' | sort -u )
NPROD=$(grep -c . <<< "$PRODUCED")

if [ "$NPROD" -ge 8 ]; then
  ok "A4-control the producer enumeration is sound — $NPROD candidate literals at fire sites"
else
  bad "A4-control the producer enumeration is sound — only $NPROD found (expected >= 8; the extractor broke)"
fi

# NO EXEMPTIONS. The arg sweep cannot tell an alert_type from a state_key, so
# intersect against the vocabulary itself; what bites is the one direction that
# matters — a produced type MISSING from it. An exemption list was carried here
# for one release and was wrong: the type it exempted is suppressible on its
# recovery notice, and exempting it also refused a stored value that worked.
MISSING=$(comm -23 <(printf '%s\n' "$PRODUCED" | sort -u) <(printf '%s\n' "$BACK"))
NMISSING=$(grep -c . <<< "$MISSING")
if [ "$NMISSING" -eq 0 ]; then
  ok "A4 every alert type raised at a fire site is suppressible — no exemptions"
else
  bad "A4 $NMISSING alert type(s) page an operator with no way to suppress them"
  while IFS= read -r t; do [ -n "$t" ] && printf '        ungoverned: %s\n' "$t"; done <<< "$MISSING"
fi

# A5: the raw-INSERT producer is in the vocabulary too. It reaches the alerts
# table without passing the fan-out, so a literal-argument sweep of fire_alert
# call sites cannot see it and A4 is structurally blind to it.
if grep -qE '^slow_response$' <<< "$BACK" && grep -qE '^slow_response$' <<< "$FRONT"; then
  ok "A5 the direct-INSERT producer is suppressible on both sides too"
else
  bad "A5 the direct-INSERT producer is suppressible on both sides too"
fi

# A6/A7: THE SECOND FRONTEND VOCABULARY. `Alerts.tsx` keeps its own
# `TYPE_LABELS`, and until v2.157.0 nothing compared it to anything — A1-A5
# above pin the Settings grid and never read this map. It had drifted in BOTH
# directions at once and stayed green the whole time: `flapping` was a label
# with zero backend producers, so the filter row offered a button that could
# only ever return an empty list, while `slow_response` was missing, so its
# badge printed the raw column value and the filter could not select it at all.
# The two errors could not cancel out and neither could be seen from the Settings
# arms, because that grid had `slow_response` all along.
LABELS=$(sed -n '/^const TYPE_LABELS: Record<string, string> = {/,/^};/p' "$ALERTS_TSX" \
         | grep -oE '^\s*[a-z_]+:' | tr -d ' :' | sort -u)
NLABELS=$(grep -c . <<< "$LABELS")

if [ "$NLABELS" -ge 15 ]; then
  ok "A6-control the Alerts.tsx label map extracted — $NLABELS types"
else
  bad "A6-control the Alerts.tsx label map extracted — only $NLABELS (expected >= 15; the extractor broke, the map did not shrink)"
fi

ONLY_B=$(comm -23 <(printf '%s\n' "$BACK") <(printf '%s\n' "$LABELS") | grep -c .)
ONLY_L=$(comm -13 <(printf '%s\n' "$BACK") <(printf '%s\n' "$LABELS") | grep -c .)
if [ "$ONLY_B" -eq 0 ] && [ "$ONLY_L" -eq 0 ]; then
  ok "A7 the Alerts.tsx label map names exactly the backend vocabulary"
else
  bad "A7 Alerts.tsx and the backend disagree — $ONLY_B unlabelled (raw badge, unselectable filter), $ONLY_L labelled with no producer (a filter button that returns nothing)"
  comm -23 <(printf '%s\n' "$BACK") <(printf '%s\n' "$LABELS") | while read -r t; do
    [ -n "$t" ] && printf '        no label: %s\n' "$t"; done
  comm -13 <(printf '%s\n' "$BACK") <(printf '%s\n' "$LABELS") | while read -r t; do
    [ -n "$t" ] && printf '        no producer: %s\n' "$t"; done
fi

# A8: the filter row must RENDER from that map, or A7 pins a constant nothing
# uses — the same trap A3 closes for the Settings grid.
if grep -qE 'Object\.entries\(TYPE_LABELS\)\.map\(' "$ALERTS_TSX"; then
  ok "A8 the type filter renders from the label map, so A7 pins what the operator sees"
else
  bad "A8 the type filter renders from the label map — an inline list at the JSX site is invisible to A7"
fi

echo
echo "== §B  a token that names nothing is refused, not stored =="

# B1: scoped to upsert_rules' BODY. The file has other handlers; a whole-file
# grep would be satisfied by any of them.
UPSERT=$(awk '/^async fn upsert_rules\(/{inside=1} inside{print} inside && /^}/{exit}' "$RULES")
NUPSERT=$(grep -c . <<< "$UPSERT")
if [ "$NUPSERT" -ge 20 ]; then
  ok "B1-control upsert_rules body extracted — $NUPSERT lines"
else
  bad "B1-control upsert_rules body extracted — only $NUPSERT lines (the extractor broke)"
fi
if grep -qE 'unknown_suppressible_types' <<< "$UPSERT" \
   && grep -qE 'StatusCode::BAD_REQUEST' <<< "$UPSERT"; then
  ok "B1 the live edit path refuses a suppression token it cannot honour"
else
  bad "B1 the live edit path refuses a suppression token it cannot honour"
fi

# B2: scoped to the alert_rules import block, and asserted on the value that is
# actually BOUND. Grepping the file for the helper's name proves only that the
# call exists somewhere — the regression that matters keeps the call, keeps the
# counter, and binds the RAW value anyway, which a whole-file `has` cannot see.
IMPORTBLOCK=$(sed -n '/if let Some(rules) = body.get("alert_rules")/,/^    \/\/ Import monitors/p' "$SETTINGS_RS")
NIMP=$(grep -c . <<< "$IMPORTBLOCK")
if [ "$NIMP" -ge 40 ]; then
  ok "B2-control the alert_rules import block extracted — $NIMP lines"
else
  bad "B2-control the alert_rules import block extracted — only $NIMP lines (the extractor broke)"
fi
if grep -qE 'let muted_types = muted_owned\.as_str\(\);' <<< "$IMPORTBLOCK" \
   && [ "$(grep -cE 'let muted_types = muted_raw' <<< "$IMPORTBLOCK")" -eq 0 ] \
   && grep -qE 'muted_types_dropped \+=' <<< "$IMPORTBLOCK"; then
  ok "B2 the restore path binds the SANITISED list and counts what it dropped"
else
  bad "B2 the restore path binds the SANITISED list and counts what it dropped"
fi

# B2b: and the count reaches the caller. A drop nobody is told about is the
# silent acceptance this section exists to end, one layer further out.
if grep -qE '"muted_types_dropped": muted_types_dropped' <<< "$(cat "$SETTINGS_RS")"; then
  ok "B2b the import response reports how many tokens it dropped"
else
  bad "B2b the import response reports how many tokens it dropped"
fi

# B3: the read path filters too. Without it a stored stale token round-trips
# through the checkboxes into the payload, and B1 then makes the page
# permanently unsaveable — the fix for one defect creating another.
READBLOCK=$(sed -n '/if (r.muted_types) {/,/^        }/p' "$SETTINGS_TSX")
if [ "$(grep -c . <<< "$READBLOCK")" -ge 4 ] && grep -qE 'known\.has\(' <<< "$READBLOCK"; then
  ok "B3 the settings page drops tokens it cannot render before they reach the payload"
else
  bad "B3 the settings page drops tokens it cannot render before they reach the payload"
fi

echo
echo "== §C  an exhausted escalation chain does not go silent =="

# Scoped to check_escalations' body: alert_engine.rs is 2000 lines and holds
# several cadence checks, so a whole-file grep proves nothing about this one.
CHECKESC=$(awk '/^async fn check_escalations\(/{inside=1} inside{print} inside && /^}$/{exit}' "$ENGINE")
NESC=$(grep -c . <<< "$CHECKESC")
if [ "$NESC" -ge 100 ]; then
  ok "C1-control check_escalations body extracted — $NESC lines"
else
  bad "C1-control check_escalations body extracted — only $NESC lines (the extractor broke)"
fi

# The decision itself is a PURE function, extracted so the arithmetic can be
# reached without a pool, a policy and a firing alert. Pin it there: the loop
# below only routes what this returns.
DECIDE=$(awk '/^fn escalation_decision\(/{inside=1} inside{print} inside && /^}$/{exit}' "$ENGINE")
NDEC=$(grep -c . <<< "$DECIDE")
if [ "$NDEC" -ge 15 ]; then
  ok "C1-control escalation_decision body extracted — $NDEC lines"
else
  bad "C1-control escalation_decision body extracted — only $NDEC lines (the extractor broke)"
fi

# C1: both rungs of the unattached cadence, so a spent chain is never worse
# than no chain: nothing before 15 minutes, then every 30.
if grep -qE '< 15' <<< "$DECIDE" && grep -qE '< 30' <<< "$DECIDE"; then
  ok "C1 a spent chain re-pages on the same 15/30-minute cadence an unattached rule gets"
else
  bad "C1 a spent chain re-pages on the same 15/30-minute cadence an unattached rule gets"
fi

# C2: and it must RESOLVE to a page. The defect was that a spent chain returned
# the terminal outcome for ever; an arm that merely checks the enum exists
# cannot tell that apart, so require the spent path to yield RepeatLast.
if grep -qE '=> *EscalationAction::RepeatLast|EscalationAction::RepeatLast,?$' <<< "$DECIDE"; then
  ok "C2 a spent chain resolves to another page, not to a terminal state"
else
  bad "C2 a spent chain resolves to another page, not to a terminal state"
fi

# C3: the loop pages the LAST rung and leaves the index alone. Advancing it
# would walk off the end of the chain; the default fan-out would throw away the
# destinations the operator chose for the worst case.
REPEAT=$(sed -n '/EscalationAction::RepeatLast => {/,/^                    }$/p' <<< "$CHECKESC")
NREP=$(grep -c . <<< "$REPEAT")
if [ "$NREP" -ge 5 ]; then
  ok "C3-control the RepeatLast arm extracted — $NREP lines"
else
  bad "C3-control the RepeatLast arm extracted — only $NREP lines (the extractor broke)"
fi
if grep -qE '\(steps\[steps\.len\(\) - 1\]\.clone\(\), None\)' <<< "$REPEAT"; then
  ok "C3 the fallback pages the last rung's own route and leaves the step index put"
else
  bad "C3 the fallback pages the last rung's own route and leaves the step index put"
fi

# C3b: SEVERED-PAIR CHECK. A pure function nothing calls is a pure function
# that decides nothing — the sweep would go on doing whatever it did before
# while every unit test above stayed green.
if grep -qE 'escalation_decision\(&after, current_index' <<< "$CHECKESC"; then
  ok "C3b the sweep actually routes on that decision"
else
  bad "C3b the sweep actually routes on that decision — the pure function is orphaned"
fi

# C4: the function's own doc comment must not still promise the old behaviour.
# A breadcrumb that survives the code it described is how the next reader
# "restores" the defect. This reads the COMMENT, which `code()`-style strippers
# remove — so it is deliberately taken from the raw file.
DOC=$(sed -n '/^\/\/\/ Re-notify for unacknowledged firing alerts\./,/^async fn check_escalations/p' "$ENGINE")
if [ "$(grep -c . <<< "$DOC")" -ge 10 ] && [ "$(grep -cE 'never re-page' <<< "$DOC")" -eq 0 ]; then
  ok "C4 the doc comment no longer promises that an exhausted chain stops paging"
else
  bad "C4 the doc comment still promises that an exhausted chain stops paging"
fi

echo
echo "== §D  the operator-facing copy says what the code does =="

if grep -qE 'When the chain runs out' <<< "$(cat "$GUIDE")"; then
  ok "D1 the monitoring guide documents what happens after the last step"
else
  bad "D1 the monitoring guide documents what happens after the last step"
fi

ESCCOPY=$(sed -n '/Escalation policies/,/New policy/p' "$ALERTS_TSX")
if grep -qE 'does not go quiet' <<< "$ESCCOPY"; then
  ok "D2 the policy editor tells the operator the alert keeps paging after the last step"
else
  bad "D2 the policy editor tells the operator the alert keeps paging after the last step"
fi

# D3: internal release jargon must not reach operator copy. "pre-W3" named an
# internal phase and appeared in a paragraph an operator reads.
if [ "$(grep -cE 'pre-W3' <<< "$ESCCOPY")" -eq 0 ]; then
  ok "D3 no internal phase jargon in the operator-facing escalation copy"
else
  bad "D3 no internal phase jargon in the operator-facing escalation copy"
fi

echo
echo "== §E  a mute reaches EVERY route shape, not three of four =="

# The grid's own text names webhooks among what a mute silences. An escalation
# step routed `webhook:<url>` has no user behind it, so it was the one shape
# that consulted nobody's preference — and therefore the one shape a mute could
# not reach, while `user:` and `on_call_schedule:` steps honoured it through the
# per-user fan-out.
DISPATCH=$(awk '/^pub async fn dispatch_escalation_step\(/{inside=1} inside{print} inside && /^}$/{exit}' "$NOTIF")
NDIS=$(grep -c . <<< "$DISPATCH")
if [ "$NDIS" -ge 30 ]; then
  ok "E1-control dispatch_escalation_step body extracted — $NDIS lines"
else
  bad "E1-control dispatch_escalation_step body extracted — only $NDIS lines (the extractor broke)"
fi

# Scoped to the webhook branch itself: a mute check anywhere else in the
# function is satisfied by the fan-out the OTHER routes already use, which is
# exactly the sibling that made this gap invisible.
WEBHOOK=$(sed -n '/if let Some(url) = route.strip_prefix("webhook:")/,/^    }$/p' <<< "$DISPATCH")
NWEB=$(grep -c . <<< "$WEBHOOK")
if [ "$NWEB" -ge 10 ]; then
  ok "E1-control the webhook branch extracted — $NWEB lines"
else
  bad "E1-control the webhook branch extracted — only $NWEB lines (the extractor broke)"
fi
if grep -qE 'is_type_muted\(' <<< "$WEBHOOK"; then
  ok "E1 a step-level webhook honours the alert owner's suppression"
else
  bad "E1 a step-level webhook honours the alert owner's suppression"
fi

# E2: and it must ACT on it. The branch already ends in its own `return;` after
# sending, so an arm that merely finds a return in the branch passes with the
# guard deleted — it measures the wrong half. Scope to the guard's OWN block.
GUARD=$(sed -n '/if is_type_muted(&owner, alert_type) {/,/^            }$/p' <<< "$WEBHOOK")
NG=$(grep -c . <<< "$GUARD")
if [ "$NG" -ge 3 ] && grep -qE '^ *return;' <<< "$GUARD"; then
  ok "E2 a muted type RETURNS from inside the guard rather than being read and ignored"
else
  bad "E2 a muted type RETURNS from inside the guard — guard block is $NG line(s)"
fi

echo
echo
echo "== §F  the fired severity reaches every payload, not a keyword guess =="

# F1: send_notification_with_runbook — every alert-fire path's actual send —
# takes severity as the caller's own value and never re-derives it from the
# subject text. It used to keyword-scan "DockPanel Alert: <title>" for words
# like "FAIL"/"critical"/"warning", so a stored-critical whose title read
# plainly ("Certificate expired") reached PagerDuty as the generic "error".
SNWR=$(awk '/^pub async fn send_notification_with_runbook\(/{inside=1} inside{print} inside && /^}$/{exit}' "$NOTIF")
NSNWR=$(grep -c . <<< "$SNWR")
if [ "$NSNWR" -ge 60 ]; then
  ok "F1-control send_notification_with_runbook body extracted — $NSNWR lines"
else
  bad "F1-control send_notification_with_runbook body extracted — only $NSNWR lines (the extractor broke)"
fi
if grep -qE 'severity: &str,' <<< "$SNWR" && [ "$(grep -cE 'derive_severity\(' <<< "$SNWR")" -eq 0 ]; then
  ok "F1 the fired-alert send path takes severity as a parameter, not a subject-text guess"
else
  bad "F1 the fired-alert send path takes severity as a parameter, not a subject-text guess"
fi

# F2: dispatch_escalation_step threads that same severity to every route shape
# it can pick — the webhook synthesis plus all four fanout_to_user calls
# (all_channels, unresolved-route fallback, the per-user loop, the
# nobody-serviceable fallback). A route shape that drops it silently reverts
# to a subject-text guess one call deeper.
DISPATCH_F=$(awk '/^pub async fn dispatch_escalation_step\(/{inside=1} inside{print} inside && /^}$/{exit}' "$NOTIF")
NDISF=$(grep -c . <<< "$DISPATCH_F")
if [ "$NDISF" -ge 100 ]; then
  ok "F2-control dispatch_escalation_step body extracted — $NDISF lines"
else
  bad "F2-control dispatch_escalation_step body extracted — only $NDISF lines (the extractor broke)"
fi
NSEV=$(grep -cE '^ +severity,$' <<< "$DISPATCH_F")
if grep -qE 'severity: &str,' <<< "$DISPATCH_F" && [ "$NSEV" -ge 5 ]; then
  ok "F2 every route shape in dispatch_escalation_step passes the real severity onward — $NSEV call sites"
else
  bad "F2 dispatch_escalation_step passes severity to every route shape — only $NSEV call sites found (expected >= 5)"
fi

# F3: fanout_to_user forwards severity to the send it actually makes, rather
# than accepting it as a dead parameter.
FANOUT=$(awk '/^async fn fanout_to_user\(/{inside=1} inside{print} inside && /^}$/{exit}' "$NOTIF")
NFAN=$(grep -c . <<< "$FANOUT")
if [ "$NFAN" -ge 25 ]; then
  ok "F3-control fanout_to_user body extracted — $NFAN lines"
else
  bad "F3-control fanout_to_user body extracted — only $NFAN lines (the extractor broke)"
fi
if grep -qE 'severity: &str,' <<< "$FANOUT" && grep -qE '^ +severity,$' <<< "$FANOUT"; then
  ok "F3 fanout_to_user forwards severity to the actual send"
else
  bad "F3 fanout_to_user forwards severity to the actual send"
fi

# F4: check_escalations SELECTs the row's real severity and colours the
# escalation HTML from it, rather than hardcoding critical-red for every
# re-page — the half of the s402 escalation fix that was left open: an
# escalated warning was indistinguishable from an escalated critical.
CHECKESC_F=$(awk '/^async fn check_escalations\(/{inside=1} inside{print} inside && /^}$/{exit}' "$ENGINE")
NESCF=$(grep -c . <<< "$CHECKESC_F")
if [ "$NESCF" -ge 200 ]; then
  ok "F4-control check_escalations body extracted — $NESCF lines"
else
  bad "F4-control check_escalations body extracted — only $NESCF lines (the extractor broke)"
fi
if grep -qE 'severity: String,' <<< "$CHECKESC_F" \
   && grep -qE 'SELECT id, user_id, server_id, alert_type, severity,' <<< "$CHECKESC_F" \
   && grep -qE 'severity_color\(&row\.severity\)' <<< "$CHECKESC_F" \
   && [ "$(grep -cE '"#ef4444"' <<< "$CHECKESC_F")" -eq 0 ]; then
  ok "F4 check_escalations selects the row's severity and colours the re-page from it"
else
  bad "F4 check_escalations selects the row's severity and colours the re-page from it"
fi

# F5: try_fire_alert's initial page and check_escalations' re-page share ONE
# severity->colour mapping. Before this they were two copies of the same match
# arms in two files, free to drift the moment either one was edited alone.
if [ "$(grep -c 'pub fn severity_color(severity: &str)' "$NOTIF")" -eq 1 ]; then
  ok "F5 severity->colour is single-sourced, not duplicated per caller"
else
  bad "F5 severity->colour is single-sourced, not duplicated per caller"
fi

echo
printf 'alert-controls: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
