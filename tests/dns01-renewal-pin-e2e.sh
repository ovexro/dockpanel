#!/usr/bin/env bash
# Regression pins for the s392 ship — a certificate is renewed over the challenge
# that ISSUED it.
#
# ⛔ EVERY ARM READS A COMMENT-STRIPPED SUBJECT. s386's suite printed 43/43 green
# with that ship's headline defect restored, because the explanatory comment above
# the fix quoted the code and satisfied every arm. Stripping is not a style choice
# here; an arm that reads a file any other way is a bug in this file.
#
# WHAT THE SHIP FIXED
#
#   `sites` recorded nothing about which ACME challenge produced a certificate, so
#   all three renewal doors aimed an HTTP-01, single-name order at `site.domain`.
#   For a DNS-01 certificate that is wrong in two directions, both silent:
#
#     SHAPE A — the site IS the Cloudflare zone apex. The certificate directory is
#       named after the site, so `foreign_cert_issuer` finds the wildcard, sees a
#       Let's Encrypt issuer, and returns "not foreign" — permission to proceed.
#       `provision_cert` then overwrites the shared fullchain.pem IN PLACE with a
#       single-name certificate and every sibling vhost in the zone begins serving
#       a certificate that does not cover it. Reachable on a STOCK install:
#       `auto_fix_safe_findings` runs on every security scan with no opt-in.
#
#     SHAPE B — a subdomain under a zone wildcard. There is no certificate at
#       `/etc/dockpanel/ssl/{site.domain}/`, so the guard exits on `has_cert:false`
#       before it ever reads an issuer. The renewal writes an orphan certificate,
#       the panel re-renders the vhost from `ssl_cert_path` and points nginx BACK
#       at the un-renewed wildcard, and stamps the NEW certificate's expiry on the
#       row. The 45-day window never reopens and the site goes dark behind a panel
#       that already reported a successful renewal.
#
# §A is the arm that would have caught it: it derives the subject the ISSUANCE
# door records and the subject the RENEWAL door orders, and fails when they can
# drift apart. An arm pinned to either side alone was green throughout.
#
# Static analysis over source text: offline, deterministic, same verdict on an
# air-gapped runner (lesson #641).
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
eq()  { [ "$2" = "$3" ] && ok "$1" || bad "$1" "expected '$3', got '$2'"; }
has() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1" "missing: $3" ;; esac; }
hasnt(){ case "$2" in *"$3"*) bad "$1" "present but must not be: $3" ;; *) ok "$1" ;; esac; }

# ugrep's --ignore-files shim honours .gitignore, so use the real binary.
G=/usr/bin/grep

subj()   { sed -E -e 's://.*$::' -e 's:^[[:space:]]*--.*$::' "$1" | tr -d ' \n\\'; }
subjin() { sed -E -e 's://.*$::' -e 's:^[[:space:]]*--.*$::' | tr -d ' \n\\'; }
# ⛔ ONE FUNCTION'S BODY, not the file (#672). A file-scoped `has` asks "does this
# file contain this shape anywhere", which for ordinary Rust is always yes.
fnbody() { awk -v p="$2" 'index($0,p){f=1} f{print} f && /^}$/{exit}' "$1"; }
occ()    { printf '%s' "$1" | $G -oF -- "$2" | wc -l | tr -d ' '; }

HELP=panel/backend/src/helpers.rs
BSSL=panel/backend/src/routes/ssl.rs
SITES=panel/backend/src/routes/sites.rs
HEAL=panel/backend/src/services/auto_healer.rs
SCAN=panel/backend/src/services/security_scanner.rs
AGENT=panel/backend/src/services/agent.rs
ASSL=panel/agent/src/services/ssl.rs
ASSLR=panel/agent/src/routes/ssl.rs
MIGDIR=panel/backend/migrations
MIG="$MIGDIR/20260823000000_ssl_provenance.sql"

for f in "$HELP" "$BSSL" "$SITES" "$HEAL" "$SCAN" "$AGENT" "$ASSL" "$ASSLR" "$MIG"; do
  [ -f "$f" ] || { bad "SETUP" "$f missing"; exit 1; }
done

# An arm over an empty subject prints green for every absence below (#143), so
# every subject is floored on its POST-strip size before anything reads it.
for pair in "$HELP:20000" "$BSSL:20000" "$SITES:60000" "$HEAL:30000" "$SCAN:8000" \
            "$AGENT:20000" "$ASSL:12000" "$ASSLR:9000" "$MIG:900"; do
  f=${pair%:*}; min=${pair##*:}
  n=$(subj "$f" | wc -c)
  [ "$n" -ge "$min" ] || { bad "SETUP" "$f has $n chars of code, expected >= $min"; exit 1; }
done

F_HELP=$(subj "$HELP"); F_BSSL=$(subj "$BSSL"); F_SITES=$(subj "$SITES")
F_HEAL=$(subj "$HEAL"); F_SCAN=$(subj "$SCAN"); F_AGENT=$(subj "$AGENT")
F_ASSL=$(subj "$ASSL"); F_ASSLR=$(subj "$ASSLR"); F_MIG=$(subj "$MIG")

echo "── §A  the door that ISSUES and the door that RENEWS agree on the subject"

# ⭐ THE ARM THAT WOULD HAVE CAUGHT THE ORIGINAL DEFECT. Both halves are DERIVED
# from source and compared: an arm pinned to either side alone stayed green for
# 137 releases while the two disagreed.
#
# The issuance door records the name it ORDERED (`provision_domain`, which on a
# wildcard is the zone, not the site). The renewal door must order the name the
# row RECORDED (`ssl_cert_subject`). If either side ever names `site.domain`
# instead, they have drifted and the wildcard is destroyed again.
has "A1 the DNS-01 door records the name it ordered, not the site" "$F_BSSL" \
    "ssl_challenge='dns-01',ssl_cert_subject=\$6"
has "A1b bound to provision_domain — the ZONE on a wildcard" "$F_BSSL" \
    ".bind(provision_domain)"
has "A2 the HTTP-01 door records the site's own domain" "$F_BSSL" \
    "ssl_challenge='http-01',ssl_cert_subject=\$6"
F_RENEWDNS=$(fnbody "$BSSL" "pub(crate) async fn renew_over_dns01(" | subjin)
[ "$(printf '%s' "$F_RENEWDNS" | wc -c)" -ge 800 ] || \
  { bad "SETUP" "renew_over_dns01 body extracted to $(printf '%s' "$F_RENEWDNS" | wc -c) chars — arms over it examine nothing"; exit 1; }
has "A3 the renewal orders the RECORDED subject" "$F_RENEWDNS" \
    'format!("/ssl/provision-dns01/{subject}")'
# …and never the site's own domain, which is what the defect did.
hasnt "A3b and never the site's own domain" "$F_RENEWDNS" "site.domain"

# The agent route this reuses ALREADY existed and already is a renewal — that is
# why the fix reaches an installed fleet with no agent upgrade. Pinned so a
# future edit cannot quietly invent a second route and strand every existing box.
has "A4 the agent route reused here is the one that already shipped" "$F_ASSLR" \
    '.route("/ssl/provision-dns01/{domain}",post(provision_dns01))'
has "A4b and it orders the wildcard SAN off the same flag" "$F_ASSL" \
    'ids.push(Identifier::Dns(format!("*.{domain}")));'

echo "── §B  the plan is decided from POSITIVE evidence, never from an absence"

F_PLAN=$(fnbody "$HELP" "pub async fn renewal_plan(" | subjin)
[ "$(printf '%s' "$F_PLAN" | wc -c)" -ge 900 ] || \
  { bad "SETUP" "renewal_plan body extracted to $(printf '%s' "$F_PLAN" | wc -c) chars"; exit 1; }

# ⭐ THE RULE THAT CONVERGES. Renewing over HTTP-01 requires the row to SAY
# `http-01`. Absence-of-wildcard-evidence does not converge: a NON-wildcard
# DNS-01 certificate is byte-identical to an HTTP-01 one in every field the panel
# or the agent can read — one identifier, no `*.` SAN, its own directory — and
# that is the population whose operator chose DNS-01 *because port 80 cannot be
# reached*. Under an absence rule the fleet would re-order those over HTTP-01 for
# ever. Under this rule an unrecorded row degrades exactly ONCE and records itself.
has "B1 positive evidence of http-01 renews over http-01" "$F_PLAN" \
    'Some("http-01")=>returnRenewalPlan::Http01{record_challenge:false}'
has "B2 an unrecorded row degrades ONCE and records itself" "$F_PLAN" \
    'None=>returnRenewalPlan::Http01{record_challenge:true}'
# Everything else — including `dns-01` — falls through to the DNS-01 branch.
has "B3 every other recorded challenge falls through" "$F_PLAN" '_=>{}'

# ⛔ THE FORGED SIGNAL. A draft of the s392 ship derived provenance from the
# stored certificate path and would have branded healthy sites as wildcards and
# refused to renew them for ever. The plan must never read that column.
#
# ⚠ The reason originally written here — "rename_domain writes only the domain
# while the agent MOVES the certificate directory" — was repealed by the very
# release that wrote it: the rename now rewrites that column itself. The signal
# is STILL forged, for two reasons that do survive: the rewrite is scoped to the
# site's own old directory, so a wildcard child renamed out of its zone keeps
# naming the zone; and any row renamed on a build at or below v2.144.x keeps the
# stale path, whose name can be re-claimed by the next site that asks for it.
# Recorded because an arm defended by a dead reason is one refactor from being
# deleted as obsolete.
hasnt "B4 the plan never reads ssl_cert_path (rename forges it)" "$F_PLAN" "ssl_cert_path"
# POSITIVE CONTROL: the same subject DOES carry the columns it is supposed to read,
# so B4 cannot be passing because the extraction is empty or misspelled.
has "B4-control the plan does read the recorded challenge" "$F_PLAN" "ssl_challenge"
has "B4-control2 and the recorded subject" "$F_PLAN" "ssl_cert_subject"

echo "── §C  whose Cloudflare credential renews whose certificate"

# ⛔ An unattended loop has no actor. Resolving a zone by bare domain would hand
# one account's Cloudflare token to a renewal running for another account's site,
# and every other `dns_zones` read in the tree is scoped `user_id = $2`.
has "C1 the legacy fallback scopes zones to the SITE OWNER" "$F_PLAN" \
    "WHEREuser_id=\$1ANDprovider='cloudflare'"
has "C1b bound to the site's owner, not an actor" "$F_PLAN" ".bind(site.user_id)"
# The recorded zone ROW is preferred, so the credential that renews is provably
# the credential that issued and no text subject can select a different tenant's.
has "C2 the recorded zone id is preferred" "$F_PLAN" "site.ssl_dns_zone_id"
has "C2b and the issuance door records it" "$F_BSSL" ".bind(zone.id)"
# The unattended loops take the key from the registry field, not the environment.
# One each: the healer opens it at the renewal, the scanner threads it from `run`.
eq "C3 both loops open the token through the registry accessor" \
   "$(( $(occ "$F_HEAL" "agents.jwt_secret()") + $(occ "$F_SCAN" "agents.jwt_secret()") ))" "2"
has "C3b the scanner threads it rather than re-reading it" "$F_SCAN" "jwt_secret:&str,"

echo "── §D  refusal is a RUNG, not a terminus"

# For a zone-apex wildcard the OLD silent downgrade at least left the apex
# serving; refusing all the way to expiry takes the apex AND every sibling down
# together. So a refusal that cannot be repaired in time becomes a DELIBERATE,
# recorded, alerted downgrade instead — never worse than the behaviour it replaced.
has "D1 the ladder has a last rung" "$F_HELP" "pubconstDNS01_LAST_RESORT_DAYS:i64=7;"
has "D2 the rung is reached by time remaining" "$F_PLAN" \
    "days_remaining.is_some_and(|d|d<=last_resort)"
has "D2a the rung is chosen PER PROFILE, not from the bare constant" "$F_PLAN" \
    "dns01_last_resort_days(site.ssl_profile.as_deref())"

# ⭐ D2b — THE INVARIANT, which is what actually failed. The rung is only
# reachable if it sits STRICTLY BELOW the profile's renewal margin: renewal does
# not begin until the margin, so a rung at or above it means `Refuse` is never
# returned for that profile and the ladder silently loses its top step.
#
# It failed for `shortlived` from the day the profile shipped. A `shortlived`
# certificate is ~160 hours, so `num_days()` of the remainder is at most SIX and
# a rung of seven was never exceeded — the whole profile went straight to the
# single-name downgrade, and the operator was never warned while the zone was
# still repairable. The old constant's own docstring named the counterexample it
# was refuted by ("sits under every profile's fallback margin: 2 / 15 / 30").
#
# Pinned as a COMPARISON of the two tables rather than as literals, so the arm
# survives a deliberate re-tuning of either number and still fails the moment
# they cross. Both tables are asserted non-empty first: a parse that finds
# nothing would otherwise compare zero pairs and pass (#143).
LR_TBL=$(sed -n '/pub fn dns01_last_resort_days/,/^}/p' "$HELP")
FM_TBL=$(sed -n '/fn fallback_renewal_margin/,/^}/p' "$HEAL")
lr_for() { # $1=profile ; falls through to the default arm like the match does
  local v
  v=$(printf '%s' "$LR_TBL" | grep -oE "Some\(\"$1\"\) => [0-9]+" | grep -oE '[0-9]+$')
  [ -n "$v" ] && { printf '%s' "$v"; return; }
  printf '%s' "$(grep -oE 'pub const DNS01_LAST_RESORT_DAYS: i64 = [0-9]+' "$HELP" | grep -oE '[0-9]+$')"
}
fm_for() {
  local v
  v=$(printf '%s' "$FM_TBL" | grep -oE "Some\(\"$1\"\) => chrono::Duration::days\([0-9]+\)" | grep -oE '[0-9]+')
  [ -n "$v" ] && { printf '%s' "$v"; return; }
  printf '%s' "$(printf '%s' "$FM_TBL" | grep -oE '_ => chrono::Duration::days\([0-9]+\)' | grep -oE '[0-9]+')"
}
if [ "$(printf '%s' "$LR_TBL" | wc -c)" -lt 60 ] || [ "$(printf '%s' "$FM_TBL" | wc -c)" -lt 60 ]; then
  bad "D2b-control both rung tables parse" "last-resort=$(printf '%s' "$LR_TBL" | wc -c)B margin=$(printf '%s' "$FM_TBL" | wc -c)B"
else
  ok "D2b-control both rung tables parse ($(printf '%s' "$LR_TBL" | wc -c)B / $(printf '%s' "$FM_TBL" | wc -c)B)"
  D2B_OK=1; D2B_MSG=""
  for prof in shortlived tlsserver classic; do
    lr=$(lr_for "$prof"); fm=$(fm_for "$prof")
    if [ -z "$lr" ] || [ -z "$fm" ] || [ "$lr" -ge "$fm" ]; then
      D2B_OK=0; D2B_MSG="$D2B_MSG $prof(rung=${lr:-?} margin=${fm:-?})"
    fi
  done
  if [ "$D2B_OK" -eq 1 ]; then
    ok "D2b every profile's rung sits below its renewal margin, so Refuse is reachable"
  else
    bad "D2b every profile's rung sits below its renewal margin" "unreachable for:$D2B_MSG"
  fi
fi
has "D3 and it names what stops being covered" "$F_PLAN" "RenewalPlan::LastResortHttp01{losing:names}"
has "D3b the wildcard's names are spelled out for the operator" "$F_PLAN" \
    'format!("{subject}and*.{subject}")'

# ⛔ A DECLINE IS NOT A FAILURE. `ssl_renewal_alert` / `ssl_renewal_blocked` say
# "renewal failed" and page at critical; both are false when the panel declined on
# purpose. `ssl-correctness` pins the failure helpers' counts for exactly this
# reason, with a comment saying so. These arms pin the other end.
eq "D4 each loop has its own decline helper" \
   "$(( $(occ "$F_HEAL" "asyncfnssl_dns01_declined_alert(") + $(occ "$F_SCAN" "asyncfnssl_dns01_declined_alert(") ))" "2"
eq "D5 and its own downgrade helper" \
   "$(( $(occ "$F_HEAL" "asyncfnssl_dns01_downgraded_alert(") + $(occ "$F_SCAN" "asyncfnssl_dns01_downgraded_alert(") ))" "2"
# The decline warns; the downgrade pages. Getting these the wrong way round is
# the defect `ssl-controls` F13 exists to catch, one layer out.
F_HDECL=$(fnbody "$HEAL" "async fn ssl_dns01_declined_alert(" | subjin)
F_HDOWN=$(fnbody "$HEAL" "async fn ssl_dns01_downgraded_alert(" | subjin)
[ "$(printf '%s' "$F_HDECL" | wc -c)" -ge 200 ] && [ "$(printf '%s' "$F_HDOWN" | wc -c)" -ge 200 ] || \
  { bad "SETUP" "alert helper bodies extracted too small"; exit 1; }
has "D6 the decline is a warning" "$F_HDECL" '"warning",'
has "D7 the downgrade is critical" "$F_HDOWN" '"critical",'
hasnt "D8 and the downgrade never calls itself a failure" "$F_HDOWN" "renewalfailed"

echo "── §E  the budget is derived from the agent, not guessed"

# ⛔ The three doors disagreed: two used plain `post` (a hard 60s cap) and the
# interactive one `post_long(..,120)`, while a wildcard DNS-01 order budgets
# ~260s inside the agent alone. Every door timed out while the agent SUCCEEDED,
# and the panel recorded a failure and wrote a cooldown row.
has "E1 one budget, named once" "$F_BSSL" "pub(crate)constDNS01_ORDER_TIMEOUT_SECS:u64=300;"
# ⭐ WIDENED at s393, from "every renewal door" to EVERY DNS-01 door. v2.145.0 gave
# the three renewal doors the shared budget and left the door that ISSUES a
# wildcard — the exact order the constant derives its arithmetic from — passing a
# bare literal 180. The constant's own doc comment already said "every caller must
# use THIS constant"; the arm counted only the callers that did.
# ⭐ 6 -> 7 at v2.161.0: the scanner gained a SECOND renewal door, for a Compose
# stack whose domain resolves to no site. It spends the shared budget like every
# other door, so the total moves. The right-hand side is a typed literal, so a
# new door legitimately moves this arm — bumping it is the repair, and changing
# the panel to dodge it would be the defect.
eq "E2 and every DNS-01 door spends THAT budget — issuance included" \
   "$(( $(occ "$F_BSSL" "DNS01_ORDER_TIMEOUT_SECS") + $(occ "$F_HEAL" "DNS01_ORDER_TIMEOUT_SECS") + $(occ "$F_SCAN" "DNS01_ORDER_TIMEOUT_SECS") ))" "7"
# ⛔ A COUNT OF THE CONSTANT'S USES CANNOT SEE A CALLER THAT DOES NOT USE IT —
# that is how the issuance door stayed invisible to E2 for a whole release (#671:
# a count is blind to the thing it does not count). E2b asks the complementary
# question: EVERY long agent call in this file spends the shared budget.
#
# ⚠ BOTH SIDES ARE DERIVED FROM THE SUBJECT. Neither is a number typed here, so
# the arm cannot become a tautology when the population grows (#701) — add a
# fourth door and it stays green only if that door also names the constant.
#
# ⚠ The first draft of this arm was `$G -cE 'post_long\([^)]*,[0-9]+\)'` = 0, and
# a mutation proved it could NOT fail: the subject is whitespace-stripped, so the
# call ends `,180,)` with Rust's trailing comma, and `&format!(…)` puts a `)`
# INSIDE the argument list that `[^)]*` stops at. #626, rebuilt inside the suite
# written to catch it. A regex over an argument list is not a parser; count the
# calls instead.
eq "E2b every long agent call in this file spends THAT budget" \
   "$(occ "$F_BSSL" ",DNS01_ORDER_TIMEOUT_SECS")" "$(occ "$F_BSSL" "post_long(")"
# ⭐ DERIVED FROM THE AGENT'S OWN ARITHMETIC, not from a number typed here: the
# per-authorization propagation sleep, and the two poll budgets. If the agent ever
# waits longer than the panel is willing to, this goes red before a fleet does.
AGENT_SLEEP=$(printf '%s' "$F_ASSL" | $G -oE 'sleep\(std::time::Duration::from_secs\([0-9]+\)\)' | $G -oE '[0-9]+' | sort -rn | head -1)
AGENT_POLL=$(printf '%s' "$F_ASSL" | $G -oE 'lettimeout=std::time::Duration::from_secs\([0-9]+\)' | $G -oE '[0-9]+' | sort -rn | head -1)
[ -n "$AGENT_SLEEP" ] && [ -n "$AGENT_POLL" ] || \
  { bad "SETUP" "could not derive the agent's own waits (sleep='$AGENT_SLEEP' poll='$AGENT_POLL')"; exit 1; }
AGENT_WORST=$(( 2 * AGENT_SLEEP + 2 * AGENT_POLL ))
# ⛔ THE PANEL'S SIDE IS DERIVED TOO. An earlier draft of this arm compared the
# agent's derived worst case against the literal `300` TYPED HERE — which cannot
# fail however the constant is edited, because the arm was reading its own
# assumption instead of the value under test. Mutation caught it: dropping the
# real constant to 120 reddened E1 and left E3 green. Both sides derived, or the
# comparison is decoration ([[feedback_verifier_shares_source]]).
PANEL_BUDGET=$(printf '%s' "$F_BSSL" | $G -oE 'DNS01_ORDER_TIMEOUT_SECS:u64=[0-9]+' | $G -oE '[0-9]+$' | head -1)
[ -n "$PANEL_BUDGET" ] || { bad "SETUP" "could not derive the panel's own budget"; exit 1; }
if [ "$PANEL_BUDGET" -ge "$AGENT_WORST" ]; then
  ok "E3 the panel's budget (${PANEL_BUDGET}s) covers the agent's own worst case (${AGENT_WORST}s = 2x${AGENT_SLEEP}s + 2x${AGENT_POLL}s)"
else
  bad "E3 the panel's budget covers the agent's own worst case" \
      "panel ${PANEL_BUDGET}s < agent ${AGENT_WORST}s — a DNS-01 renewal reports a false failure"
fi
# The unattended door must not fall back to the 60s `post`.
hasnt "E4 the healer no longer spends the 60s quick budget on a certificate order" "$F_HEAL" \
    'agent.post(&agent_path,Some(agent_body)).await'

echo "── §F  provenance is written where it is known, and only on success"

# All five writers, so a sixth door cannot appear without a decision.
# Counted PER FILE and per value, not lumped: the three writers mean different
# things and a lumped total is invariant under moving one to the wrong place.
eq "F1 the DNS-01 door is the only writer of 'dns-01'" \
   "$(occ "$F_BSSL" "ssl_challenge='dns-01',")" "1"
# TWO in this file: the HTTP-01 issuance door, and `record_renewal_provenance`,
# which is what makes an unrecorded row correct after one degraded pass and what
# tells the truth after a deliberate last-resort downgrade.
eq "F1b routes/ssl.rs writes 'http-01' at the door and at the recorder" \
   "$(occ "$F_BSSL" "ssl_challenge='http-01',")" "2"
eq "F1c and the auto-SSL-on-create door writes it once" \
   "$(occ "$F_SITES" "ssl_challenge='http-01',")" "1"
eq "F2 and the retiring doors CLEAR it" \
   "$(( $(occ "$F_BSSL" "ssl_challenge=NULL,") + $(occ "$F_SITES" "ssl_challenge=NULL,") ))" "2"
# ⛔ ONLY AFTER SUCCESS. A failed HTTP-01 attempt on an unrecorded row must stay
# unrecorded: the likeliest reason it failed is that the site cannot answer
# HTTP-01 at all, which is exactly the case that made DNS-01 right. Recording
# `http-01` there would pin the wrong answer for ever.
eq "F3 every door records provenance after a renewal" \
   "$(( $(occ "$F_BSSL" "record_renewal_provenance(") + $(occ "$F_HEAL" "record_renewal_provenance(") + $(occ "$F_SCAN" "record_renewal_provenance(") ))" "4"
F_REC=$(fnbody "$BSSL" "pub(crate) async fn record_renewal_provenance(" | subjin)
[ "$(printf '%s' "$F_REC" | wc -c)" -ge 500 ] || \
  { bad "SETUP" "record_renewal_provenance body extracted too small"; exit 1; }
# ⛔ TWO DIFFERENT QUESTIONS. "Does this renewal change what is RECORDED about
# provenance?" is true for two plans. "Did it install a certificate at the site's
# OWN directory?" is true for all THREE HTTP-01 plans. s393's first draft gated
# the path on the provenance question and left the COMMONEST population — a row
# already stamped `http-01` whose stored path names somewhere else — permanently
# uncorrectable. v2.145.0 manufactured exactly that shape. Found by driving a box.
has "F4 the recorded-provenance plans are the two that change what is true" "$F_REC" \
    "RenewalPlan::Http01{record_challenge:true}=>(true,None),"
has "F4b and the deliberate downgrade" "$F_REC" \
    "RenewalPlan::LastResortHttp01{losing}=>(true,Some(losing.clone())),"
has "F4c an ORDINARY recorded renewal still reaches the path write" "$F_REC" \
    "RenewalPlan::Http01{record_challenge:false}=>(false,None),"
has "F4d and only DNS-01 writes nothing — a wildcard's path names the ZONE" "$F_REC" "_=>return,"
# The downgrade leaves a durable record the operator can filter for.
has "F5 the downgrade is logged at a level the readers can count" "$F_REC" '"warning",'

# ⭐ s393 — THE PATH FOLLOWS THE CERTIFICATE. No renewal door writes the stored
# certificate path (0 occurrences in either service file; both DO write
# `SET ssl_expiry`, so that is a measurement and not a missing grep), yet every
# door re-renders the vhost FROM that column moments later. A row whose path
# named a different directory — a wildcard child naming the zone — therefore had
# its certificate renewed and nginx pointed straight back at the un-renewed
# wildcard, with the NEW expiry stamped on the row. The window never reopened.
# Both plans that reach this function ordered HTTP-01 for the site's own name, so
# the certificate really is at the site's own directory.
has "F6 the recorder moves the path" "$F_REC" \
    "ssl_cert_path=\$2,ssl_key_path=\$3,"
# ⛔ AND UNGATED. The path write must not sit inside the provenance branch — that
# is the defect this arm exists to prevent, and it is invisible to F6 alone.
case "${F_REC%%ifrecord_provenance*}" in
  *"ssl_cert_path=\$2,ssl_key_path=\$3,"*)
     ok "F6c the path write precedes the provenance branch, so every HTTP-01 plan reaches it" ;;
  *) bad "F6c the path write precedes the provenance branch" \
         "it is gated on provenance — a row already stamped http-01 keeps its stale path for ever" ;;
esac
has "F6b bound to the directory the HTTP-01 order actually wrote" "$F_REC" \
    'format!("/etc/dockpanel/ssl/{domain}/fullchain.pem")'
# ⚠ F6b's literal appears elsewhere in this file. It is sound ONLY because
# `$F_REC` is a `fnbody` extraction (#672 — a file-scoped `has` asks "does this
# file contain this shape anywhere", which for ordinary Rust is always yes).
# Re-scope it to `$F_BSSL` and it becomes satisfiable by a sibling and dies silently.

# ⛔ ORDER, NOT PRESENCE. Recording the path after the rebuild would leave the
# rebuild reading the stale value — the defect unchanged, with both strings
# present and every `has` above still green. Only an ordering arm can see it.
# Derived by splitting each subject at its rebuild and asking what precedes it.
# ⚠ Passed by NAME, never interpolated into a delimited string: a stripped Rust
# body is full of `::`, so a colon-delimited loop would split the subject itself.
records_before_rebuild() {  # $1 label  $2 subject  $3 rebuild marker
  case "$2" in
    *"$3"*)
      case "${2%%"$3"*}" in
        *"record_renewal_provenance("*)
          ok "F7-$1 records provenance BEFORE it re-renders the vhost" ;;
        *) bad "F7-$1 records provenance BEFORE it re-renders the vhost" \
               "the rebuild runs first, so it re-asserts the stale path" ;;
      esac ;;
    *) bad "F7-$1 subject does not contain $3" "arm is vacuous — re-scope it" ;;
  esac
}
# ⚠ `renew` is a thin wrapper that delegates; the body that renews — and that the
# Diagnostics "Fix" button also reaches — is `renew_for_site`. Pinning the wrapper
# extracts 460 chars containing neither call, and both arms below would be vacuous.
F_RENEW=$(fnbody "$BSSL" "pub(crate) async fn renew_for_site(" | subjin)
[ "$(printf '%s' "$F_RENEW" | wc -c)" -ge 2000 ] || \
  { bad "SETUP" "renew_for_site body extracted to $(printf '%s' "$F_RENEW" | wc -c) chars"; exit 1; }
records_before_rebuild BSSL "$F_RENEW" "rebuild_vhost_after_ssl("
records_before_rebuild HEAL "$F_HEAL" "build_nginx_body("
records_before_rebuild SCAN "$F_SCAN" "build_nginx_body("

# ⛔ s393 — REVOKE RE-RENDERS THE VHOST IT JUST STRIPPED. The agent's teardown
# deletes `/etc/dockpanel/ssl/{domain}/` while the vhost still names it, and
# `nginx -t` is WHOLE-SERVER: every later site edit on the box fails and the next
# restart leaves nginx down for every tenant. The agent's shared-directory guard
# does not cover this door — it skips the site's OWN vhost — so a solo site's
# directory really is removed. Four sibling SSL writers have always re-rendered;
# this one never did. Reachable from an ordinary admin button, two clicks.
F_REV=$(fnbody "$BSSL" "pub async fn revoke(" | subjin)
[ "$(printf '%s' "$F_REV" | wc -c)" -ge 700 ] || \
  { bad "SETUP" "revoke body extracted to $(printf '%s' "$F_REV" | wc -c) chars"; exit 1; }
has "F8 revoke re-renders the vhost it just stripped of its certificate" "$F_REV" \
    "rebuild_vhost_after_ssl(&state,&agent,id).await;"
has "F8-control the subject really is the revoke door" "$F_REV" "ssl_challenge=NULL,"
# ⛔ AND AFTER THE CLEAR. `build_nginx_body` emits the certificate keys only under
# `if site.ssl_enabled`, and this helper re-reads the row — so called BEFORE the
# clear it writes the HTTPS config back naming the deleted directory, fails
# `nginx -t`, and is rolled back. That is a silent no-op which satisfies F8.
case "${F_REV%%rebuild_vhost_after_ssl(*}" in
  *"ssl_enabled=false,"*) ok "F8b and only after the row has been cleared" ;;
  *) bad "F8b and only after the row has been cleared" \
         "the rebuild precedes the UPDATE — it re-asserts the deleted certificate and is rolled back" ;;
esac

echo "── §G  the migration cannot run before the schema it alters"

# ⛔ sqlx parses a migration's version prefix as an i64 and runs them in ASCENDING
# numeric order, with no out-of-order guard. A 13-digit prefix sorts BELOW every
# 14-digit one, so it would run before `initial.sql` — `ALTER TABLE sites` against
# a database with no `sites` table, killing every FRESH install while an upgrading
# box stays green. A draft of this ship had exactly that filename. Nothing in the
# tree validated the shape.
BADNAME=0
for m in "$MIGDIR"/*.sql; do
  b=$(basename "$m")
  case "$b" in
    [0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]_*) ;;
    *) BADNAME=$((BADNAME+1)); LASTBAD="$b" ;;
  esac
done
MIGCOUNT=$(find "$MIGDIR" -maxdepth 1 -name '*.sql' | wc -l | tr -d ' ')
[ "$MIGCOUNT" -ge 100 ] || { bad "SETUP" "only $MIGCOUNT migrations found — refusing to judge"; exit 1; }
if [ "$BADNAME" -eq 0 ]; then
  ok "G1 all $MIGCOUNT migration filenames carry a 14-digit version prefix"
else
  bad "G1 every migration filename carries a 14-digit version prefix" \
      "$BADNAME do not, e.g. ${LASTBAD:-?} — sqlx would order it before the schema it alters"
fi

# ⛔ THE BACKFILL'S SOURCE. `activity_logs` is a positive historical fact written
# by the DNS-01 door itself; `ssl_cert_path` is forged by every rename. If a
# future edit swaps the source, this goes red.
has "G2 the backfill reads the activity log" "$F_MIG" "FROMactivity_logs"
has "G2b for the actions the DNS-01 door writes" "$F_MIG" "'site.ssl.dns01','site.ssl.wildcard'"
has "G2c and refuses a domain that is ambiguous across servers" "$F_MIG" "HAVINGcount(*)=1"
# Three-state on purpose: a NOT NULL DEFAULT FALSE would assert "not a wildcard"
# over every legacy apex wildcard — the one row this migration cannot identify.
hasnt "G3 ssl_wildcard is nullable, so 'not recorded' is representable" "$F_MIG" \
    "ssl_wildcardBOOLEANNOTNULL"
has "G3-control the column is added at all" "$F_MIG" "ADDCOLUMNIFNOTEXISTSssl_wildcardBOOLEAN"

echo "── §H  the pin harness can see the code it judges"

# ⛔ `ssl-correctness`'s `prod_lines` blanks from the FIRST `#[cfg(test)]` to EOF
# and never resumes, so production code below one is invisible to every `prod_*`
# arm. `resolve_profile` had drifted below it and was blinded at v2.144.0 — the
# second instance of lesson #669. This arm is the standing guard.
FIRSTTEST=$($G -n '^#\[cfg(test)\]' "$BSSL" | head -1 | cut -d: -f1)
[ -n "$FIRSTTEST" ] || { bad "SETUP" "no #[cfg(test)] found in $BSSL"; exit 1; }
PRODBELOW=$(awk -v s="$FIRSTTEST" 'NR>s && /^(pub|async fn|fn |impl |struct |const |static )/' "$BSSL" | wc -l | tr -d ' ')
if [ "$PRODBELOW" -eq 0 ]; then
  ok "H1 no production item sits below the first #[cfg(test)] in routes/ssl.rs"
else
  bad "H1 no production item sits below the first #[cfg(test)] in routes/ssl.rs" \
      "$PRODBELOW item(s) are blinded to every prod_* arm in ssl-correctness"
fi
# POSITIVE CONTROL: the same scan DOES find production items ABOVE the marker, so
# H1 cannot be passing because the pattern matches nothing at all.
PRODABOVE=$(awk -v s="$FIRSTTEST" 'NR<s && /^(pub|async fn|fn |impl |struct |const |static )/' "$BSSL" | wc -l | tr -d ' ')
[ "$PRODABOVE" -ge 10 ] && ok "H1-control the same scan finds $PRODABOVE production items above it" \
  || bad "H1-control the same scan finds production items above it" "only $PRODABOVE — the pattern matches nothing"

echo
echo "── dns01-renewal: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
