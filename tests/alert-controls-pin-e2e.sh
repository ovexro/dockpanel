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
SEC_SCANS=panel/backend/src/routes/security_scans.rs
IMG_SCANS=panel/backend/src/routes/image_scans.rs
RULES=panel/backend/src/routes/alerts.rs
SETTINGS_RS=panel/backend/src/routes/settings.rs
SETTINGS_TSX=panel/frontend/src/pages/Settings.tsx
ALERTS_TSX=panel/frontend/src/pages/Alerts.tsx
GUIDE=docs/guides/monitoring.md

for f in "$NOTIF" "$ENGINE" "$HEALER" "$SCANNER" "$SEC_SCANS" "$IMG_SCANS" "$RULES" "$SETTINGS_RS" \
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
GRIDBLOCK=$(sed -n '/Alert Behaviour/,/GPU Alert Thresholds/p' "$SETTINGS_TSX")
NGRID=$(grep -c . <<< "$GRIDBLOCK")
if [ "$NGRID" -ge 20 ]; then
  ok "A3-control the behaviour grid extracted — $NGRID lines"
else
  bad "A3-control the behaviour grid extracted — only $NGRID lines (the extractor broke; the heading or the card below it was renamed)"
fi
if grep -qE 'SUPPRESSIBLE_ALERT_TYPES\.map\(' <<< "$GRIDBLOCK" \
   && [ "$(grep -cE 'key: "[a-z_]+", label:' <<< "$GRIDBLOCK")" -eq 0 ]; then
  ok "A3 the grid renders from the shared constant, with no inline list at the JSX site"
else
  ok_inline=$(grep -cE 'key: "[a-z_]+", label:' <<< "$GRIDBLOCK")
  bad "A3 the grid renders from the shared constant — $ok_inline inline entr(ies) found at the JSX site"
fi

# A3b: BOTH columns render, and each is bound to its own source. The grid used
# to be a single mute checkbox per type; a regression that drops the Record
# column leaves A3 green (the map still renders) while the seven columns go
# back to being unreachable outside Export/Import Config, which is the exact
# state this replaced.
if grep -qE 'RECORD_COLUMN_BY_TYPE\[key\]' <<< "$GRIDBLOCK" \
   && grep -qE 'checked=\{recorded\}' <<< "$GRIDBLOCK" \
   && grep -qE 'checked=\{!mutedTypes\.includes\(key\)\}' <<< "$GRIDBLOCK"; then
  ok "A3b both columns render — Record from the column map, Notify from the mute list"
else
  bad "A3b both columns render — Record from the column map, Notify from the mute list"
fi

# A3c: every record column the backend reads has a row in the frontend map.
# This reads is_alert_enabled's OWN match arms, not the map, so the two cannot
# agree with each other vacuously. A column the backend honours but the grid
# never offers is a switch the operator cannot reach — the defect this shipped
# to fix, one type at a time.
BACKCOLS=$(awk '/pub async fn is_alert_enabled\(/{i=1} i{print} i && /^}$/{exit}' "$NOTIF" \
           | grep -oE '=> *"alert_[a-z_]+"' | grep -oE 'alert_[a-z_]+' | sort -u)
NBC=$(grep -c . <<< "$BACKCOLS")
MAPCOLS=$(sed -n '/^const RECORD_COLUMN_BY_TYPE: Record<string, string> = {/,/^};/p' "$SETTINGS_TSX" \
          | grep -oE '"alert_[a-z_]+"' | tr -d '"' | sort -u)
NMC=$(grep -c . <<< "$MAPCOLS")
if [ "$NBC" -ge 7 ] && [ "$NMC" -ge 7 ]; then
  ok "A3c-control both column enumerations are sound — backend $NBC, grid map $NMC"
else
  bad "A3c-control both column enumerations are sound — backend $NBC, grid map $NMC (expected >= 7 each; an extractor broke)"
fi
UNREACHABLE=$(comm -23 <(printf '%s\n' "$BACKCOLS") <(printf '%s\n' "$MAPCOLS") | grep -c .)
if [ "$UNREACHABLE" -eq 0 ]; then
  ok "A3c every record column the backend honours is reachable from the grid"
else
  bad "A3c $UNREACHABLE record column(s) the backend honours have no control in the grid"
  comm -23 <(printf '%s\n' "$BACKCOLS") <(printf '%s\n' "$MAPCOLS") | while read -r c; do
    [ -n "$c" ] && printf '        unreachable: %s\n' "$c"; done
fi

# A4: THE REAL INVARIANT. Every alert type handed to the fire_alert family must
# be suppressible. This reads the PRODUCERS, not either list, so it cannot be
# satisfied by the lists agreeing with each other.
PRODUCED=$( { calls "$ENGINE" fire_alert_with_retry
              calls "$ENGINE" 'notifications::try_fire_alert'
              calls "$HEALER" 'notifications::fire_alert_deduped'
              calls "$SCANNER" 'notifications::fire_alert_deduped'
              calls "$SCANNER" 'notifications::fire_alert'
              calls "$SEC_SCANS" 'notifications::fire_alert'
              calls "$SEC_SCANS" 'notifications::resolve_alert'
              calls "$IMG_SCANS" 'notifications::fire_alert'
              calls "$IMG_SCANS" 'notifications::resolve_alert'; } 2>/dev/null \
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
echo "== §G  switching a type off also stops ESCALATING it =="

# The switch had exactly one caller — inside try_fire_alert — so it governed
# whether a row was ever RECORDED and said nothing about a row already firing
# when the operator flipped it. This sweep selects on status='firing' alone, so
# the one control meaning "do not tell me about this" went on re-paging every
# thirty minutes until the row aged out of the seven-day window.
CHECKESC_G=$(awk '/^async fn check_escalations\(/{i=1} i{print} i && /^}$/{exit}' "$ENGINE")
NESCG=$(grep -c . <<< "$CHECKESC_G")
if [ "$NESCG" -ge 200 ]; then
  ok "G1-control check_escalations body extracted — $NESCG lines"
else
  bad "G1-control check_escalations body extracted — only $NESCG lines (the extractor broke)"
fi

# G1: POSITIONAL, not presence. An arm that merely finds the call passes with
# the guard moved BELOW the dispatch, where it decides nothing — and passes
# just as well with the whole sweep replaced by an early return. Require the
# consult to precede the page, inside this body.
ENAB_LINE=$(grep -n 'is_alert_enabled(' <<< "$CHECKESC_G" | head -1 | cut -d: -f1)
DISP_LINE=$(grep -n 'dispatch_escalation_step(' <<< "$CHECKESC_G" | head -1 | cut -d: -f1)
if [ -n "$ENAB_LINE" ] && [ -n "$DISP_LINE" ] && [ "$ENAB_LINE" -lt "$DISP_LINE" ]; then
  ok "G1 the sweep consults the per-type switch (line $ENAB_LINE) before it pages (line $DISP_LINE)"
else
  bad "G1 the sweep consults the per-type switch before it pages — consult=${ENAB_LINE:-none} page=${DISP_LINE:-none}"
fi

# G2: and ACTS on it. The guard's own block must skip the row; a read that
# falls through pages the alert anyway.
GBLOCK=$(sed -n '/if !enabled {/,/^        }$/p' <<< "$CHECKESC_G")
NGB=$(grep -c . <<< "$GBLOCK")
if [ "$NGB" -ge 3 ]; then
  ok "G2-control the disabled-type guard block extracted — $NGB lines"
else
  bad "G2-control the disabled-type guard block extracted — only $NGB lines (the extractor broke)"
fi
if grep -qE '^ *continue;' <<< "$GBLOCK"; then
  ok "G2 a switched-off type skips the row rather than being read and ignored"
else
  bad "G2 a switched-off type skips the row rather than being read and ignored"
fi

# G3: the skip must NOT stamp escalated_at. Stamping would rotate the row to
# the back of the least-recently-paged ordering — which is what terminal_ids is
# for — but escalated_at means "this row was paged at this time", and writing
# it for a page that never went out is the stamp-claiming-the-operator-was-told
# harm maintenance_users documents in this same file. A re-enabled type must
# page on the NEXT tick, not thirty minutes after one.
# Paired with a positive control: terminal_ids must still be stamped SOMEWHERE
# in the body, or this arm cannot tell "correctly absent here" from "the
# rotation was deleted outright".
if [ "$(grep -cE 'terminal_ids\.push' <<< "$GBLOCK")" -eq 0 ]; then
  ok "G3 the skip does not stamp escalated_at for a page that never went out"
else
  bad "G3 the skip stamps escalated_at — a stamp claiming the operator was paged"
fi
if [ "$(grep -cE 'terminal_ids\.push' <<< "$CHECKESC_G")" -ge 1 ]; then
  ok "G3-control the terminal rotation still exists elsewhere in the sweep"
else
  bad "G3-control the terminal rotation still exists elsewhere in the sweep — G3 cannot distinguish absence from deletion"
fi

# G4: the consult is memoized per tick, like the policy/steps/runbook caches
# beside it. Firing rows cluster on a few keys and this sweep is capped at 500,
# so an unmemoized lookup is up to 500 extra round-trips on the shared pool
# every sixty seconds — the self-amplifying load the bounded sweep exists to
# prevent, reintroduced one layer up.
if grep -qE 'enabled_cache' <<< "$CHECKESC_G" \
   && [ "$(grep -cE 'enabled_cache\.insert' <<< "$CHECKESC_G")" -ge 1 ]; then
  ok "G4 the per-type consult is memoized per tick, not re-queried per row"
else
  bad "G4 the per-type consult is memoized per tick, not re-queried per row"
fi

echo
echo "== §H  the switch can still say YES =="

# §G proves the sweep CONSULTS the switch and skips when it says no. Every one
# of those arms is a refusal, and a guard is not proven by its refusals: with
# is_alert_enabled returning false on every path — nothing recorded, nothing
# escalated, every alert in the product silently gone — §A through §G were 50/0
# GREEN. That is #805 exactly, so the positive half is pinned here.
ENAB=$(awk '/^pub async fn is_alert_enabled\(/{i=1} i{print} i && /^}$/{exit}' "$NOTIF")
NEN=$(grep -c . <<< "$ENAB")
if [ "$NEN" -ge 30 ]; then
  ok "H1-control is_alert_enabled body extracted — $NEN lines"
else
  bad "H1-control is_alert_enabled body extracted — only $NEN lines (the extractor broke)"
fi

# H1: POSITIONAL. The column lookup must actually be REACHED — an early
# unconditional return above it makes every arm below decorative while leaving
# them all green.
MATCH_LINE=$(grep -n 'let column = match alert_type' <<< "$ENAB" | head -1 | cut -d: -f1)
if [ -n "$MATCH_LINE" ]; then
  PRE=$(sed -n "1,${MATCH_LINE}p" <<< "$ENAB")
  NPRERET=$(grep -cE '^\s*return\b' <<< "$PRE")
else
  NPRERET=-1
fi
if [ "$MATCH_LINE" ] && [ "$NPRERET" -eq 0 ]; then
  ok "H1 the column lookup is reached — nothing returns above it"
else
  bad "H1 the column lookup is reached — match at line ${MATCH_LINE:-none}, $NPRERET early return(s) above it"
fi

# H2: THE NARROWING BOUNDARY. A type this function has no column for must
# answer TRUE, or §G's skip silently stops escalating the twelve types that
# were never switchable — slow_response, security, ssl_renewal_failure,
# container_* and the rest. That is the difference between "the operator turned
# this off" and "we stopped paging for two thirds of the catalogue".
if grep -qE '_ => return true,' <<< "$ENAB"; then
  ok "H2 a type with no column answers TRUE — the twelve unswitchable types still escalate"
else
  bad "H2 a type with no column answers TRUE — the twelve unswitchable types still escalate"
fi

# H3: and an operator who has never opened the settings page has NO alert_rules
# row at all. That must read as enabled; unwrap_or(false) would silence every
# alert for every such user, which is the same product-wide silence as H1 by a
# quieter door. Asserted on the body's LAST expression, not anywhere in it.
TAIL=$(grep -vE '^\s*(//|$)' <<< "$ENAB" | tail -2 | head -1)
if grep -qE 'result\.map\(\|r\| r\.0\)\.unwrap_or\(true\)' <<< "$TAIL"; then
  ok "H3 no stored rule reads as ENABLED, and that is the body's final answer"
else
  bad "H3 no stored rule reads as ENABLED as the body's final answer — got: $TAIL"
fi

echo
printf 'alert-controls: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
