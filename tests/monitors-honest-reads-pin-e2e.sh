#!/usr/bin/env bash
# monitors-honest-reads-pin-e2e.sh — s407 / v2.159.0
#
# Pins two properties of the monitoring routes, both of which had failed in the
# same direction: the reassuring answer.
#
#   §A  the four reads in `monitors.rs` REPORT a failed query instead of
#       answering it with an empty list. All four discarded the error, so a
#       failure rendered as an emptiness the operator cannot tell from a real
#       one: "No SSL certificates found" on the certificate page, a public
#       status page with no monitors on it, an empty response chart, and — the
#       consequential one — a maintenance list showing no windows while
#       `uptime.rs` was still skipping every monitor a live window covers.
#   §B  the certificate ladder can say RENEWAL FAILED. Every rung was a
#       function of the clock, so a certificate whose renewal is failing read
#       `ok` until its last month. v2.157.0 made `ssl_renewal_failure` resolve
#       itself on a successful renewal, which sharpened the gap rather than
#       closing it: a failure that is fixed now disappears, and one that is not
#       had no rung to appear in.
#   §C  the rung's SOURCE. The keys come from the shared list filtered through
#       the shared predicate, never re-listed here — a second copy of that
#       classification is free to drift from the first. Scoped to firing rows
#       only, because `resolve_alert` clears only firing rows and an
#       acknowledged alert would pin the rung on for ever.
#   §D  every ladder call site passes the new argument, including the host-scan
#       half that must pass `false` — those certificates have no site row, so
#       nothing can raise a renewal alert against them.
#   §E  the page renders the rung, and stops reporting a failed read as an
#       empty list.
#
# Pure source analysis: no box, no network, no build.
#
# ⚠ EVERY arm reads STRIPPED source. This ship wrote long comments into both
# subjects, and they name the very literals these arms match — `renewal_failed`,
# the firing status, the predicate's own name. Against raw text most of §B and
# §C would be satisfied by the prose that describes them and would stay green
# with the code deleted (the trap in feedback_source_pin_prose_trap).
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on
# its first match and the arm goes red on correct code.
#
# MUTATION-TESTED at HEAD — see the run recorded in the ship's ledger.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

MON=panel/backend/src/routes/monitors.rs
NOTIF=panel/backend/src/services/notifications.rs
CERTS=panel/frontend/src/pages/Certificates.tsx

for f in "$MON" "$NOTIF" "$CERTS"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT. The naive block-comment strip deletes real code
# because the opener occurs inside string literals, and a truncated subject
# makes an ABSENCE arm pass on code the stripper merely removed.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
  ' "$1"
}

subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }

# The body of one top-level fn, bounded by the NEXT top-level fn. A fixed
# `grep -A n` window is not a function.
fnbody() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / {
      if ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(") { inside=1; next }
      inside=0
    }
    inside { print }
  ' <<< "$1"
}

# Every call to $2 in text $1, one per line, arguments PAREN-BALANCED and
# whitespace-flattened. Immune to rustfmt reflowing a call across lines.
calls() {
  perl -0777 -ne '
    while (/\b\Q'"$2"'\E\s*(\((?:[^()]++|(?1))*\))/gs) {
      my $c = $1; $c =~ s/\s+/ /g; print "$c\n";
    }
  ' <<< "$1"
}

M=$(subj "$MON") || { bad "monitors.rs stripped to nothing"; M=""; }
N=$(subj "$NOTIF") || { bad "notifications.rs stripped to nothing"; N=""; }
C=$(subj "$CERTS") || { bad "Certificates.tsx stripped to nothing"; C=""; }

echo "== §A  a failed read is reported, not answered with an empty list =="

# ⛔ CONTROL, AND ITS NUMBER IS THE ARM. Every §A arm below is scoped to a
# function body; a body that stops being found yields the empty string and every
# assertion over it passes for a file that no longer contains the code. This
# floors the whole subject first, and PRINTS the count so an implausible one is
# visible rather than merely ticked.
A_LINES=$(wc -l <<< "$M")
if [ "$A_LINES" -ge 600 ]; then
  ok "A-control monitors.rs stripped subject is $A_LINES lines"
else
  bad "A-control monitors.rs stripped subject is only $A_LINES lines — subject lost"
fi

# The four reads, and the distinct context each one reports under. A shared
# context string would make one arm satisfiable by another's site.
for pair in \
  "response_chart:response chart points" \
  "status_page:status page monitors" \
  "certificate_dashboard:certificate list" \
  "list_maintenance:maintenance windows"
do
  fn=${pair%%:*}; ctx=${pair#*:}
  B=$(fnbody "$M" "$fn")
  BL=$(wc -l <<< "$B")
  if [ "$BL" -lt 5 ]; then
    bad "A-body $fn — body extracted as $BL lines, subject lost"
    continue
  fi
  # POSITIVE: the body reaches a fallible read AND maps its error under its own
  # name. Not "does not swallow" — that it REPORTS.
  if grep -qF 'fetch_all' <<< "$B" && grep -qF "internal_error(\"$ctx\"" <<< "$B"; then
    ok "A1 $fn reports a failed read as \"$ctx\" ($BL-line body)"
  else
    bad "A1 $fn reports a failed read as \"$ctx\" ($BL-line body)"
  fi
done

# REFUSAL, whole-file: the swallow is gone from every read in the file, not just
# the four above. Whole-file is the strongest form of this one — the literal
# must appear nowhere.
if grep -qF 'unwrap_or_default' <<< "$M"; then
  bad "A2 no read in monitors.rs discards its error"
else
  ok "A2 no read in monitors.rs discards its error"
fi

# CONTROL for A2: an absence arm needs a positive control, or it cannot tell
# "the swallow is gone" from "the reads are gone".
A_READS=$(grep -c 'fetch_all\|fetch_optional\|fetch_one' <<< "$M")
if [ "$A_READS" -ge 10 ]; then
  ok "A2-control monitors.rs still performs $A_READS reads"
else
  bad "A2-control monitors.rs performs only $A_READS reads — A2 may be vacuous"
fi

echo "== §B  the ladder can say the renewal is failing =="

L=$(fnbody "$M" "expiry_status")
LL=$(wc -l <<< "$L")
if [ "$LL" -ge 6 ] && [ "$LL" -le 40 ]; then
  ok "B-control expiry_status body is $LL lines"
else
  bad "B-control expiry_status body is $LL lines — expected a small match block"
fi

# POSITIVE: the ladder takes the second axis at all.
if grep -qE 'fn expiry_status\(days_left: Option<i64>, renewal_failing: bool\)' <<< "$M"; then
  ok "B1 the ladder takes a renewal-failure argument"
else
  bad "B1 the ladder takes a renewal-failure argument"
fi

# POSITIVE: it produces the rung.
if grep -qF '"renewal_failed"' <<< "$L"; then
  ok "B2 the ladder yields a renewal_failed rung"
else
  bad "B2 the ladder yields a renewal_failed rung"
fi

# ORDER, and it is the whole design. `expired` is decided BEFORE the flag is
# consulted (the outage already happened and no longer turns on the renewal),
# and the flag is consulted BEFORE the clock rungs (they say when it dies; this
# says nothing is coming to save it). Offsets inside the body, on stripped code.
P_EXPIRED=$(grep -n '"expired"' <<< "$L" | head -1 | cut -d: -f1)
P_FLAG=$(grep -n 'renewal_failing' <<< "$L" | head -1 | cut -d: -f1)
P_UNKNOWN=$(grep -n '"unknown"' <<< "$L" | head -1 | cut -d: -f1)
P_CRIT=$(grep -n '"critical"' <<< "$L" | head -1 | cut -d: -f1)
if [ -n "$P_EXPIRED" ] && [ -n "$P_FLAG" ] && [ -n "$P_UNKNOWN" ] && [ -n "$P_CRIT" ]; then
  if [ "$P_EXPIRED" -lt "$P_FLAG" ]; then
    ok "B3 expired outranks the renewal rung (line $P_EXPIRED before $P_FLAG)"
  else
    bad "B3 expired outranks the renewal rung (line $P_EXPIRED before $P_FLAG)"
  fi
  if [ "$P_FLAG" -lt "$P_UNKNOWN" ] && [ "$P_FLAG" -lt "$P_CRIT" ]; then
    ok "B4 the renewal rung outranks unknown and the clock rungs"
  else
    bad "B4 the renewal rung outranks unknown and the clock rungs"
  fi
else
  bad "B3/B4 could not locate all four rungs in the ladder body — refusing to compare"
fi

# POSITIVE, and the compatibility property: the four clock rungs all survive, so
# a caller passing `false` gets exactly the ladder that shipped before.
MISSING=""
for rung in '"unknown"' '"expired"' '"critical"' '"warning"' '"ok"'; do
  grep -qF "$rung" <<< "$L" || MISSING="$MISSING $rung"
done
if [ -z "$MISSING" ]; then
  ok "B5 all five original rungs survive beside the new one"
else
  bad "B5 rungs lost from the ladder:$MISSING"
fi

echo "== §C  where the rung's truth comes from =="

H=$(fnbody "$M" "renewal_failing_sites")
HL=$(wc -l <<< "$H")
if [ "$HL" -ge 15 ]; then
  ok "C-control renewal_failing_sites body is $HL lines"
else
  bad "C-control renewal_failing_sites body is $HL lines — subject lost"
fi

# POSITIVE: the key set is DERIVED from the shared list through the shared
# predicate. This is the arm that matters — a re-listed literal set here would
# be a second classification of the same six keys, free to drift from the one in
# notifications.rs the moment a key is added.
if grep -qF 'ssl_renewal_key::ALL' <<< "$H" && grep -qF 'renewal_success_clears' <<< "$H"; then
  ok "C1 the keys are filtered from the shared list by the shared predicate"
else
  bad "C1 the keys are filtered from the shared list by the shared predicate"
fi

# REFUSAL + BOUNDARY: the three standing conditions are never named in this
# file. `DECLINED` is somebody else's certificate, `MAIL_HOST_CONFLICT` is a
# renewal deliberately refused, `DNS01_DOWNGRADED` is one that succeeded with
# fewer names — rendering any of them as a failure reports one the panel did not
# have. Naming any key here at all is the drift C1 exists to prevent.
STANDING=""
for k in 'DECLINED' 'MAIL_HOST_CONFLICT' 'DNS01_DOWNGRADED' 'dns01_downgraded' 'mail_host_conflict'; do
  grep -qF "$k" <<< "$M" && STANDING="$STANDING $k"
done
if [ -z "$STANDING" ]; then
  ok "C2 monitors.rs re-lists no renewal key of its own"
else
  bad "C2 monitors.rs names renewal keys directly:$STANDING"
fi

# CONTROL for C2: the shared list really does carry those keys, so C2 is an
# assertion about THIS file rather than about a vocabulary that no longer exists.
K_ALL=$(grep -c 'DECLINED\|MAIL_HOST_CONFLICT\|DNS01_DOWNGRADED' <<< "$N")
if [ "$K_ALL" -ge 3 ]; then
  ok "C2-control notifications.rs carries the standing keys on $K_ALL lines"
else
  bad "C2-control notifications.rs carries the standing keys on $K_ALL lines"
fi

# POSITIVE: firing rows only, and by site. `resolve_alert` clears only firing
# rows, so an acknowledged alert is never resolved by a later success — reading
# it here would pin the rung on permanently for anyone who acknowledged before
# fixing the cause.
if grep -qF "status = 'firing'" <<< "$H"; then
  ok "C3 the lookup reads firing rows only"
else
  bad "C3 the lookup reads firing rows only"
fi
if grep -qF 'site_id = ANY($1)' <<< "$H"; then
  ok "C4 the lookup is scoped by the site ids it was given"
else
  bad "C4 the lookup is scoped by the site ids it was given"
fi

# POSITIVE: the helper's failure is reported like every other read in §A. A
# lookup that answers "not failing" when it could not read is the same defect
# this whole ship is about, one layer in.
if grep -qF 'internal_error("renewal failure lookup"' <<< "$H"; then
  ok "C5 a failed lookup is reported, not read as \"not failing\""
else
  bad "C5 a failed lookup is reported, not read as \"not failing\""
fi

# ⛔ C6 IS THE ARM THAT SURVIVES THE MUTATION THE REST OF §C SURVIVES. C1-C5 are
# every one of them still true when this lookup returns an empty set on every
# path — keys still filtered, rows still firing-scoped, failure still reported —
# while the rung becomes unreachable for every certificate in the product.
# Asserted on the body's LAST expression, so returning an empty set goes red
# however carefully the query above it is preserved.
H_LAST=$(grep -v '^[[:space:]]*$' <<< "$H" | tail -2 | head -1)
if grep -qF 'rows' <<< "$H_LAST"; then
  ok "C6 the returned set is built from the rows the query read — \"$(sed 's/^ *//' <<< "$H_LAST")\""
else
  bad "C6 the returned set is built from the rows the query read — got \"$(sed 's/^ *//' <<< "$H_LAST")\""
fi

echo "== §D  every ladder call site passes the second axis =="

# `calls` prints the ARGUMENT LIST of each occurrence, one per line — and the
# DECLARATION is an occurrence too. Its parameter list is the one carrying the
# type annotation, so that is what separates it from the three real call sites.
# ⚠ The first cut of this section matched on the function NAME and asserted a
# count of 3; it reported 4 and made D1 and D3 match nothing at all — two
# vacuous arms hiding behind a control whose number said so.
SITES=$(calls "$M" "expiry_status" | grep -vF 'Option<i64>')
N_SITES=$(grep -c . <<< "$SITES")
# ⛔ READ THIS NUMBER. Four call sites: the per-caller list, the admin list, the
# host-scan half, and — since v2.162.0 — the OFFLINE half, which lists a stack's
# certificate from `docker_stacks.ssl_expiry` when the agent cannot be asked at
# all. A count that drifts means a caller was added or lost, and every arm below
# would still be describing only the ones it can see.
if [ "$N_SITES" -eq 4 ]; then
  ok "D-control the ladder has $N_SITES call sites"
else
  bad "D-control the ladder has $N_SITES call sites — expected 3"
fi

# REFUSAL: no call site takes the clock alone. The compiler already refuses a
# one-argument call, so this arm's real job is to stay honest about the SHAPE
# the other §D arms match against — it goes red if `calls` ever stops resolving
# argument lists and starts printing something else.
ONE_ARG=$(grep -cx '(days_left)' <<< "$SITES")
if [ "$ONE_ARG" -eq 0 ]; then
  ok "D1 no call site passes the clock alone"
else
  bad "D1 $ONE_ARG call site(s) pass the clock alone"
fi

# POSITIVE: two of the four consult the lookup; the other two pass `false`, and
# for reasons that are NOT the same — which is why D3b exists beside D3 rather
# than the count being quietly raised. The host-scan half has no site row to ask
# about. The offline half has no agent to ask at all, so even a live answer is
# out of reach; it is the panel reading back its own record.
# ⚠ A count arm cannot tell the two apart — both spell `(days_left, false)`. If
# one of them ever grew an answer it could have consulted, only a named arm
# would notice.
N_LOOKUP=$(grep -cx '(days_left, failing.contains(id))' <<< "$SITES")
N_FALSE=$(grep -cx '(days_left, false)' <<< "$SITES")
if [ "$N_LOOKUP" -eq 2 ]; then
  ok "D2 both site-backed lists consult the lookup ($N_LOOKUP sites)"
else
  bad "D2 both site-backed lists consult the lookup — found $N_LOOKUP"
fi
if [ "$N_FALSE" -eq 2 ]; then
  ok "D3 the two site-less halves pass false, having no site row to ask about"
else
  bad "D3 the two site-less halves pass false — found $N_FALSE such site(s)"
fi
# The offline half's OWN arm. `stale` is the one thing that row carries and no
# other row does, and it is the honest half of the bargain: a row assembled from
# the panel's stored date must never be mistaken for a live read of the box.
if [ "$(printf '%s' "$M" | /usr/bin/grep -oF '"stale": true,' | wc -l | tr -d ' ')" -eq 1 ]; then
  ok "D3b the offline half marks its rows as the panel's record, not a live read"
else
  bad "D3b the offline half does not mark its rows stale — an operator cannot tell a stored date from a fresh one"
fi

# POSITIVE: the two site-backed lists actually CALL the lookup, so D2's value
# came from the query rather than from a literal. One declaration plus two
# calls; the declaration is again the occurrence carrying the type annotation.
LOOKUPS=$(calls "$M" "renewal_failing_sites" | grep -vF '&[Uuid]')
N_CALLS=$(grep -c . <<< "$LOOKUPS")
if [ "$N_CALLS" -eq 2 ]; then
  ok "D4 both lists call the lookup ($N_CALLS call sites)"
else
  bad "D4 both lists call the lookup — found $N_CALLS"
fi

echo "== §E  the page shows the rung, and stops calling a failure an emptiness =="

E_LINES=$(wc -l <<< "$C")
if [ "$E_LINES" -ge 150 ]; then
  ok "E-control Certificates.tsx stripped subject is $E_LINES lines"
else
  bad "E-control Certificates.tsx stripped subject is only $E_LINES lines — subject lost"
fi

# POSITIVE: the rung has a badge. Without this entry the status falls through
# the map's default and renders as "Unknown" — the page would receive the answer
# and decline to show it.
if grep -qE 'renewal_failed: \{.*label: "Renewal failed"' <<< "$C"; then
  ok "E1 the page renders a Renewal failed badge"
else
  bad "E1 the page renders a Renewal failed badge"
fi

# POSITIVE: the count line asks about the error FIRST. On failure the list is
# still empty, so this line used to answer "No SSL certificates found" — the
# page's most reassuring sentence — beside the banner saying the read had failed.
if grep -qF 'Certificate list unavailable' <<< "$C"; then
  ok "E2 a failed read is reported as unavailable, not as zero certificates"
else
  bad "E2 a failed read is reported as unavailable, not as zero certificates"
fi

E_UNAVAIL=$(grep -n 'Certificate list unavailable' <<< "$C" | head -1 | cut -d: -f1)
E_NONE=$(grep -n 'No SSL certificates found' <<< "$C" | head -1 | cut -d: -f1)
if [ -n "$E_UNAVAIL" ] && [ -n "$E_NONE" ] && [ "$E_UNAVAIL" -lt "$E_NONE" ]; then
  ok "E3 the failure branch is decided before the empty one (line $E_UNAVAIL before $E_NONE)"
else
  bad "E3 the failure branch is decided before the empty one"
fi

# POSITIVE: a later success retracts the failure. Without this the banner and
# E2's line both outlive the request they describe, and toggling the admin
# checkbox is the ordinary way to reach that state.
R=$(perl -0777 -ne 'print $1 if /\.then\(\(data\)\s*=>\s*\{(.*?)\}\)/s' <<< "$C")
if grep -qF 'setError("")' <<< "$R"; then
  ok "E4 a successful reload clears the previous failure"
else
  bad "E4 a successful reload clears the previous failure"
fi

echo
printf 'PASS %d  FAIL %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
