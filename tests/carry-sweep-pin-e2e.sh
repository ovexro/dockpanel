#!/usr/bin/env bash
# Regression pins for the s387 carry sweep — five defects the code documented
# about itself, and a sixth found while closing the fifth.
#
# ⛔ EVERY ARM READS A COMMENT-STRIPPED SUBJECT. s386's suite printed 43/43 green
# with that ship's headline defect restored, because the explanatory comment above
# the fix quoted the SQL and satisfied every arm. Stripping is not a style choice
# here; an arm that reads a file any other way is a bug in this file.
#
# WHAT THE SHIP FIXED
#
#   * A CERTIFICATE ORDER THAT FAILED VALIDATION ANSWERED WITH A REFERENCE NUMBER.
#     The agent reports a lost ACME order as a 500 and the panel hides every agent
#     5xx behind an incident id — correctly, since the same route answers 500 for
#     an unreadable ACME account. So the commonest SSL failure of all (the domain
#     does not point here yet) cost two minutes of spinner and said nothing, while
#     the agent had already named the cause. §A pins the positive identification
#     and — the arm that matters — pins that the account fault is NOT taken with
#     it. §B pins that BOTH doors use it: provisioning fires more often than
#     renewal and the register named only renewal.
#   * A REMOTE LONG OPERATION TOOK THE QUICK SEMAPHORE, so on a fleet the limit
#     meant to stop builds starving the panel bounded only the panel's own host.
#     THREE entry points, not the two on record. §C derives the population.
#   * THE PERMIT WAIT SAT OUTSIDE THE BUDGET IT WAS SPENDING. The 270s restore
#     budget is deliberately under the 300s server timeout so the panel owns the
#     cut-off and can explain it; queueing outside that budget handed the cut-off
#     back to the server, which answers a bodyless 504. §D.
#   * AN UPLOADED CERTIFICATE INHERITED THE RETIRED ONE'S ACME PROFILE. §F.
#   * DELETING A DATABASE LEFT ITS DUMPS FOR EVER — and the fix's obvious guard was
#     WRONG: the generic route validator demands an alphanumeric first character
#     while a database name may begin with an underscore, so a purge built on it
#     would skip exactly the directories nobody can otherwise reach. §G pins the
#     charset, pins that a purge is REFUSED when another row still answers to the
#     name, and pins that the purge is not recursive.
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

# THE SUBJECT: comments removed, then whitespace and the `\` continuing a Rust
# string literal, so a multi-line statement flattens to the token it compiles to.
subj() { sed -E -e 's://.*$::' -e 's:^[[:space:]]*--.*$::' "$1" | tr -d ' \n\\'; }
subjin() { sed -E -e 's://.*$::' -e 's:^[[:space:]]*--.*$::' | tr -d ' \n\\'; }
# ⛔ ONE FUNCTION'S BODY, not the file. An arm about a fall-through CANNOT be
# written against the whole file: `(StatusCode::INTERNAL_SERVER_ERROR,
# Json(json!({"error": e})),)` is what every unremarkable map_err in a route file
# looks like, so a file-scoped arm is satisfied by its own siblings and cannot
# fail however the classifier is rewritten. Found by mutation: inverting the
# default left SIX other occurrences standing and the arm stayed green.
fnbody() { awk -v p="$2" 'index($0,p){f=1} f{print} f && /^}$/{exit}' "$1"; }
occ()  { printf '%s' "$1" | $G -oF -- "$2" | wc -l | tr -d ' '; }

ERR=panel/backend/src/error.rs
BSSL=panel/backend/src/routes/ssl.rs
ASSL=panel/agent/src/services/ssl.rs
ASSLR=panel/agent/src/routes/ssl.rs
AGENT=panel/backend/src/services/agent.rs
SITES=panel/backend/src/routes/sites.rs
DBR=panel/backend/src/routes/databases.rs
ADBS=panel/agent/src/services/database_backup.rs
ADBR=panel/agent/src/routes/database_backup.rs
AMOD=panel/agent/src/routes/mod.rs
DASH=panel/frontend/src/pages/Dashboard.tsx
DBTSX=panel/frontend/src/pages/Databases.tsx

for f in "$ERR" "$BSSL" "$ASSL" "$ASSLR" "$AGENT" "$SITES" "$DBR" "$ADBS" "$ADBR" "$AMOD" \
         "$DASH" "$DBTSX"; do
  [ -f "$f" ] || { bad "SETUP" "$f missing"; exit 1; }
done

# An arm over an empty subject prints green for every absence below (lesson #143),
# and stripping can itself eat a file (the s1093 blocks-vs-lines trap), so each
# subject is floored on its POST-strip size before anything reads it.
for pair in "$ERR:6000" "$BSSL:10000" "$ASSL:8000" "$ASSLR:5000" "$AGENT:20000" \
            "$SITES:60000" "$DBR:20000" "$ADBS:8000" "$ADBR:4000" "$AMOD:2000" \
            "$DASH:40000" "$DBTSX:30000"; do
  f=${pair%:*}; min=${pair##*:}
  n=$(subj "$f" | wc -c)
  [ "$n" -ge "$min" ] || { bad "SETUP" "$f has $n chars of code, expected >= $min"; exit 1; }
done

F_ERR=$(subj "$ERR");   F_BSSL=$(subj "$BSSL"); F_ASSLR=$(subj "$ASSLR")
F_ASSL=$(subj "$ASSL")
F_AGENT=$(subj "$AGENT"); F_SITES=$(subj "$SITES")
F_DBR=$(subj "$DBR");   F_ADBS=$(subj "$ADBS"); F_ADBR=$(subj "$ADBR")
F_AMOD=$(subj "$AMOD")
F_DASH=$(subj "$DASH"); F_DBTSX=$(subj "$DBTSX")

echo "── §A  a failed order is identified POSITIVELY, never by status alone"
has "A1 the translator exists" "$F_ERR" "pubfnacme_order_failure(e:&AgentError)->Option<String>"
F_ACMEREAD=$(fnbody "$ERR" "pub fn acme_order_failure(" | subjin)
F_DNSREAD=$(fnbody "$ERR" "pub fn dns_provider_failure(" | subjin)
has "A2 it is bounded to 5xx" "$F_ACMEREAD" "if!(500..600).contains(code){returnNone;}"
has "A2b and so is the provider reader beside it" "$F_DNSREAD" "if!(500..600).contains(code){returnNone;}"
# ⭐ THE ARM THAT MATTERS. Widening to any 5xx would reclassify an unreadable ACME
# account — a fault the operator cannot act on — as their problem to fix. The
# label and the legacy wording are the only two admissible proofs.
has "A3 the label is one proof" "$F_ERR" 'v.get("code")'
has "A4 the legacy wording is the other" "$F_ERR" "msg.starts_with(RENEWAL_FAILURE_PREFIX)"
hasnt "A5 and status alone is NOT a proof" "$F_ERR" "iftrue{"
# ⛔ SEVERED PAIR, BY CONSTRUCTION. Two crates, no shared dependency, so the same
# literal is spelled twice and renaming one silently returns every failed order to
# an incident id with nothing red. Compare the trees rather than trusting either.
LABEL='"acme_order_failed"'
eq "A6 the panel declares the label once" "$(occ "$F_ERR" "constACME_ORDER_FAILED_CODE:&str=$LABEL;")" 1
eq "A7 the agent labels in exactly one place" "$(occ "$F_ASSLR" "$LABEL")" 1
eq "A8 all THREE doors go through that one classifier" "$(occ "$F_ASSLR" "ca_or_internal(e)")" 3
# ⛔ POSITIVE IDENTIFICATION, and the direction is the whole safety of it. The
# marker is applied where the CA spoke; everything unmarked — including an arm
# added later — stays an internal fault and keeps its incident id. Inverting this
# default would hand every future error to the operator as though the CA said it.
F_CLASSIFIER=$(fnbody "$ASSLR" "fn ca_or_internal(" | subjin)
[ "$(printf '%s' "$F_CLASSIFIER" | wc -c)" -ge 200 ] || { bad "SETUP" "ca_or_internal body not found — every hasnt below would pass vacuously"; exit 1; }
has "A9a the classifier keys on the CA marker" "$F_CLASSIFIER" "ifletSome(reason)=e.strip_prefix(ssl::CA_DECLINED)"
has "A9c and on the provider marker, separately" "$F_CLASSIFIER" "ifletSome(reason)=e.strip_prefix(ssl::DNS_PROVIDER_DECLINED)"
# ⛔ THE PAIRING, not the presence. Counting each label once each is satisfied by
# a SWAP — both literals still appear exactly once while the CA marker emits the
# provider's code and vice-versa, which would make the panel tell an operator
# Let'"'"'s Encrypt declined when their API token is what refused. Neither the
# cross-tree counts nor the panel'"'"'s own unit tests can see that: the tests use
# hand-written fixtures and never read the agent. Only the pairing catches it.
has "A9d the CA marker emits the CA code" "$F_CLASSIFIER" \
    'e.strip_prefix(ssl::CA_DECLINED){return(StatusCode::INTERNAL_SERVER_ERROR,Json(serde_json::json!({"error":reason,"code":"acme_order_failed"}))'
has "A9e the provider marker emits the provider code" "$F_CLASSIFIER" \
    'e.strip_prefix(ssl::DNS_PROVIDER_DECLINED){return(StatusCode::INTERNAL_SERVER_ERROR,Json(serde_json::json!({"error":reason,"code":"dns_provider_failed"}))'
# ⛔ THE FALL-THROUGH. Neither marker matched, so no code is attached and the panel
# keeps its incident id. Inverting this default — labelling by absence — is the one
# change that would hand every future arm to the operator as though the CA spoke.
eq "A9b and an unmarked error carries no label" "$(occ "$F_CLASSIFIER" 'json!({"error":e})')" 1
hasnt "A9b2 the fall-through attaches no code of any kind" "$F_CLASSIFIER" 'json!({"error":e,"code"' 
# The marker is the OTHER severed pair — declared in the service, consumed in the
# route, and worth counting because the count is what a silent unmarking moves.
eq "A10 the marker is declared once" "$(occ "$F_ASSL" 'pubconstCA_DECLINED:&str="[ca]";')" 1
eq "A11 and marks the seventeen arms where the CA actually spoke" "$(occ "$F_ASSL" '{CA_DECLINED}')" 17
# ⛔ THE SECOND SEVERED PAIR, added when the DNS-01 door joined. Cloudflare
# refusing to publish `_acme-challenge` is the operator's to fix and earns a
# sentence, but it is NOT something the CA said — attributing it to the CA would
# name the wrong party in a message written to be trusted. Counted on both sides
# for the same reason as the first: a silent unmarking is what the count moves.
eq "A6b the panel declares the provider label once" "$(occ "$F_ERR" 'constDNS_PROVIDER_FAILED_CODE:&str="dns_provider_failed";')" 1
eq "A7b the agent labels a provider refusal in one place" "$(occ "$F_ASSLR" '"dns_provider_failed"')" 1
eq "A10b the provider marker is declared once" "$(occ "$F_ASSL" 'pubconstDNS_PROVIDER_DECLINED:&str="[dns]";')" 1
eq "A11b and marks the six arms where the provider refused" "$(occ "$F_ASSL" '{DNS_PROVIDER_DECLINED}')" 6
# ⚠ The wrapper every agent ever shipped. A panel newer than its agent recognises
# a renewal by it alone, so removing it would silently strip the compatibility path.
# ⛔ THE PAIRING IN THE SERVICE. A11/A11b count the two markers, and a COUNT cannot
# see a swap: mark one CA arm `[dns]` and one Cloudflare arm `[ca]` and both totals
# are unchanged, while the panel then tells an operator Let's Encrypt declined when
# their API token is what refused — the precise outcome the three-party split exists
# to prevent. Pin representative arms of each class to their own marker, and forbid
# the crossed forms outright.
has "A11c a CA arm carries the CA marker" "$F_ASSL" '{CA_DECLINED}TheCAdidnotvalidatetheDNS-01challenge:'
has "A11d a Cloudflare arm carries the provider marker" "$F_ASSL" '{DNS_PROVIDER_DECLINED}Cloudflarerefusedtocreatethechallengerecord:'
hasnt "A11e no CA sentence wears the provider marker" "$F_ASSL" '{DNS_PROVIDER_DECLINED}TheCA'
hasnt "A11f no Cloudflare sentence wears the CA marker" "$F_ASSL" '{CA_DECLINED}Cloudflare'
eq "A12 the renewal wrapper survives the rewrite" "$(occ "$F_ASSLR" 'format!("Renewalfailed:{msg}")')" 1

echo "── §B  BOTH SSL doors, not the one the register named"
has "B1 the shared translator exists" "$F_BSSL" "fnacme_failure_or("
eq "B2 both doors call it" "$(occ "$F_BSSL" "acme_failure_or(&site.domain,")" 2
has "B3 provisioning is one of them" "$F_BSSL" 'acme_failure_or(&site.domain,"SSLprovisioning",e))?;'
# ⚠ No trailing `;` since v2.145.0: the renewal call moved inside a `match` arm
# that branches on which challenge issued the certificate, so the expression ends
# the arm rather than a statement. The property — the renewal door propagates
# through the shared translator, not a bare passthrough — is unchanged.
has "B4 renewal is the other" "$F_BSSL" 'acme_failure_or(&site.domain,"SSLrenewal",e))?'
has "B5 it answers 422, not a passthrough" "$F_BSSL" \
    'returnerr(StatusCode::UNPROCESSABLE_ENTITY,&format!("Acertificatefor{domain}couldnotbeissued:'
# All four 422s must survive: s386's foreign-issuer refusal, s387's declined
# order, and the DNS-01 door's two — a provider refusal and a declined order,
# which are different sentences because they are different parties.
# FIVE since v2.145.0 — the DNS-01 renewal that cannot reach its Cloudflare zone
# refuses rather than silently re-ordering over HTTP-01.
# ⚠ `ssl-controls` F10b pins this SAME literal in this SAME file. Both move together.
eq "B5b and every refusal that earns a 422 keeps one" "$(occ "$F_BSSL" "StatusCode::UNPROCESSABLE_ENTITY,")" 5
# ⛔ #662. The compatibility path puts this sentence in front of a local agent
# fault too, so it must name no cause — and must never say "try again", which on
# a rate-limited order is the one instruction that makes it worse.
hasnt "B5c the sentence asserts no cause" "$F_BSSL" "LetsEncryptcouldnotissue"
hasnt "B5d and never tells the operator to retry" "$F_BSSL" "thentryagain"
F_HTTP01MSG=$(fnbody "$BSSL" "fn acme_failure_or(" | subjin)
F_DNS01MSG=$(fnbody "$BSSL" "fn dns01_failure_or(" | subjin)
[ "$(printf '%s' "$F_DNS01MSG" | wc -c)" -ge 400 ] || { bad "SETUP" "dns01_failure_or body not found — B9f would pass vacuously"; exit 1; }
has  "B5e the hint is conditional" "$F_HTTP01MSG" "Ifthatisavalidationfailure,"
# Anything not positively identified must still reach the old path untouched.
has "B6 everything else still goes to the generic mapper" "$F_BSSL" "agent_error(context,e)"
# ⛔ THE THIRD DOOR, and it must NOT share the sentence above. An operator is on
# the DNS-01 door precisely BECAUSE port 80 cannot be reached, so the sibling's
# "check that port 80 is reachable" is false by construction here. It gets its own
# translator naming the two things this door can actually fix.
has "B7 the third door has its own translator" "$F_BSSL" "fndns01_failure_or("
has "B8 and the third door is wired to it" "$F_BSSL" "dns01_failure_or(provision_domain,&zone.domain,wildcard,e))?;"
has "B9a the provider hint names the token scope" "$F_DNS01MSG" "ascopedtokenneedstheDNS:Editpermission."
has "B9b the CA hint names the challenge record" "$F_DNS01MSG" "the_acme-challengeTXTrecordfor{subject}"
# ⛔ THE ZONE, NOT THE ORDERED NAME. The zone lookup exists to match a site against
# its PARENT zone, so on the ordinary subdomain site these differ — and telling an
# operator to look in the `blog.example.com` zone sends them after something that is
# not in their Cloudflare account. Both hints must spell the resolved zone.
eq "B9d both hints name the resolved zone" "$(occ "$F_DNS01MSG" 'inthe{zone}zone')" 1
eq "B9e and the provider hint too" "$(occ "$F_DNS01MSG" 'forthe{zone}zone')" 1
hasnt "B9f and neither names the ordered name as a zone" "$F_DNS01MSG" "the{ordered}zone"
# ⛔ #663 — BOTH hints, which is what this arm's name has always claimed. The CA
# branch fires on a rate limit too, where publishing a TXT record fixes nothing, so
# an unconditional imperative there is the exact defect #663 was written about.
has "B9c the provider hint is conditional" "$F_DNS01MSG" "IfCloudflarerejectedthechange,"
has "B9c2 and so is the CA hint beside it" "$F_DNS01MSG" "Ifthatisavalidationfailure,"
# ⚠ NOT a text count. This file's own tests assert the ABSENCE of the port-80
# sentence, so their assertions carry the literal and a count would be satisfied
# by the very test meant to prove it — the source-pin prose trap. The behaviour is
# proved at RUNTIME; what a pin can honestly assert is that the proof still exists.
has "B10 and a runtime test proves it never repeats the port-80 advice" "$F_BSSL" \
    "fna_provider_refusal_names_the_token_and_never_port_80()"

echo "── §C  every remote long operation takes the LONG pool"
# ⛔ DERIVED, NOT LISTED. The register named two of these; there are three, and the
# third was found only by enumerating. An arm that hard-codes the two it was told
# about cannot see a fourth arrive (lesson #542).
RAW=$(occ "$F_AGENT" "semaphore.acquire()")
eq "C1 no entry point acquires a permit outside the shared helper" "$RAW" 0
eq "C2 the helper is defined once" "$(occ "$F_AGENT" "asyncfnpermit_within<'a>(")" 1
LONG=$(occ "$F_AGENT" "&self.cb.long_semaphore,")
QUICK=$(occ "$F_AGENT" "&self.cb.semaphore,")
eq "C3 six entry points are long-pool (3 local + 3 remote)" "$LONG" 6
eq "C4 four remain quick-pool" "$QUICK" 4
eq "C5 every acquisition goes through the helper" "$(occ "$F_AGENT" "permit_within(")" 10
# ⛔ A queued request is not an offline agent, and the small pool makes queueing
# likely. Counting a timeout as a breaker failure reports a working agent as down.
has "C6 a remote timeout is not counted against the breaker" "$F_AGENT" "ife.is_timeout(){AgentError::Request(e.to_string())}else{"
eq "C7 and every remote send is classified in one place" "$(occ "$F_AGENT" "remote_send_error(&self.cb,e)")" 4

echo "── §D  the permit wait is spent INSIDE the caller's budget"
has "D1 the helper bounds the wait by the budget" "$F_AGENT" "tokio::time::timeout(budget,sem.acquire())"
has "D2 and reports what is left of it" "$F_AGENT" "Ok((permit,budget.saturating_sub(start.elapsed())))"
# ⭐ Returning the FULL budget again would restore the defect while keeping the
# shape, so the remainder must actually reach the request.
eq "D3 the remainder is what the request is given" "$(occ "$F_AGENT" "tokio::time::timeout(remaining,")" 4
eq "D4 and what the remote requests are given" "$(occ "$F_AGENT" ".timeout(remaining)")" 3
hasnt "D5 no entry point re-derives the full budget for the call" \
      "$F_AGENT" "tokio::time::timeout(Duration::from_secs(timeout_secs),self.request_inner"
# A wait that never started is not a run that overran; #662 is why it says so.
has "D6 a queue timeout says the operation never started" "$F_AGENT" "theoperationneverstarted"

echo "── §F  an uploaded certificate does not inherit a retired profile"
has "F1 upload clears the profile" "$F_SITES" "ssl_profile=NULL,"
# Its three siblings are COALESCE-guarded precisely because keeping a known value
# beats erasing it; this column is the one where the stored value has stopped
# being true, so it must NOT join them.
hasnt "F2 and does not preserve it" "$F_SITES" "ssl_profile=COALESCE("

echo "── §G  the dump directory goes when nothing owns it — and not before"
eq "G1 the purge primitive exists" "$(occ "$F_ADBS" "pubasyncfnpurge_dir(db_name:&str)")" 1
has "G2 it validates its name first" "$F_ADBS" "if!is_db_dir_name(db_name){"
# ⭐ THE ARM THE WRONG GUARD WOULD HAVE FAILED. A database name may begin with an
# underscore; the generic route validator demands an alphanumeric first character.
# A purge built on it skips precisely the directories nobody can otherwise reach —
# and skipping is indistinguishable from finding nothing to do.
# ⛔ SUPERSET, NEVER NARROWER. The purge owning its charset is only safe if no
# name loses a door by the swap, and the length cap is exactly where that nearly
# went wrong: 63 here against 64 on the shared validator would have silently taken
# every door from a 64-character imported database. Pinned as the whole predicate,
# so a cap edit cannot pass as a formatting change.
has "G3 the predicate is exactly this — no first-char rule, no dot, capped at 64" "$F_ADBS" \
    "pubfnis_db_dir_name(name:&str)->bool{!name.is_empty()&&name.len()<=64&&name.chars().all(|c|c.is_ascii_alphanumeric()||c=='_'||c=='-')}"
# The shared validator's own cap, so the two cannot drift apart unnoticed.
has "G3b and the shared validator still caps at the same place" "$F_AMOD" "name.len()<=64"
hasnt "G4 and imposes no first-character rule" "$F_ADBS" "name.chars().next().is_some_and"
# No dot in the charset means traversal cannot be SPELLED, not merely blocked.
eq "G5 and it is declared once" "$(occ "$F_ADBS" "pubfnis_db_dir_name(")" 1
# ⛔ Not recursive. A directory holding anything else keeps its contents.
has "G6 the directory removal is the empty-only one" "$F_ADBS" "tokio::fs::remove_dir(&dir)"
hasnt "G7 nothing here removes a tree" "$F_ADBS" "remove_dir_all"
# ⛔ THE PROMISE THIS FILE MAKES OUT LOUD. The import door's predicate is an
# EXTENSION test — it asks whether a file can be READ as a dump, which is exactly
# what an operator's hand-placed `dump.sql.gz` is built to satisfy. Deleting on
# that basis would take the staged import the product's own screen tells them to
# put here. The purge must ask the narrower question: did this module MINT it.
has "G8 the purge takes only names this module minted" "$F_ADBS" "&&is_minted_dump_name(db_name,&name)"
hasnt "G8b and never the import door's extension test" "$F_ADBS" "&&is_safe_filename(&name)"
has "G8c a minted name is the database's own, timestamped" "$F_ADBS" 'name.strip_prefix(&format!("{db_name}-"))'
has "G8d with a 15-character stamp" "$F_ADBS" "stamp.len()==15"
has "G9 and only when it emptied the directory" "$F_ADBS" "ifkept==0{"
eq "G10 the route is registered" "$(occ "$F_ADBR" 'route("/db-backups/{db_name}",delete(purge))')" 1
# ⛔ THE HAZARD THE REGISTER NEVER NAMED. The directory is keyed by NAME while the
# name is unique only per SITE, so two live databases can share one. Purging on
# the first delete destroys the survivor's backups.
has "G11 the panel asks whether anything still uses the name" "$F_DBR" "SELECTEXISTS(SELECT1FROMdatabasesdJOINsitessONs.id=d.site_id"
has "G12 scoped to this host, counting an unplaced row as possibly-here" \
    "$F_DBR" "WHEREd.name=\$1AND(s.server_id=\$2ORs.server_idISNULL)"
has "G13 doubt keeps the files" "$F_DBR" "unwrap_or(true);"
has "G14 and a shared name is not purged" "$F_DBR" "ifshared{"
# The write half of the same charset split: a `_`-named database could not even be
# dumped, which is why its directory was never what went missing.
eq "G15 every dump-directory door shares one predicate" \
   "$(occ "$F_ADBR" "database_backup::is_db_dir_name(")" 6

echo "── §H  the two fixes that rode along, and the control/reader pair"
# ⛔ The AGENT is the authority on a database name and demands an alphanumeric first
# character; the panel accepted `_mydata`, forwarded it, and handed back the agent's
# raw refusal. Pin the rule on BOTH surfaces — a fix on one is the divergence again.
has "H1 the panel enforces the first-character rule" "$F_DBR" \
    ".next().is_some_and(|c|c.is_ascii_alphanumeric())"
has "H2 and the form states it rather than discovering it" "$F_DBTSX" \
    'pattern="[a-zA-Z0-9][a-zA-Z0-9_]*"'
# ⚠ The 63 is NOT the agent's 64 and must not be "aligned": a PostgreSQL identifier
# is 63 bytes, so the stricter side is the correct one and refuses in our own words.
has "H3 the length cap stays at the PostgreSQL identifier limit" "$F_DBR" "body.name.len()>63"

# ⛔ DERIVED, NOT LISTED. Every id offered as a checkbox must have a render guard
# that reads it. Three did not: two controlled nothing at all, and the certificate
# countdown was governed by the UNRELATED "Active Issues" switch — so turning off
# Active Issues silently took the SSL panel with it. An arm hard-coding today's
# fourteen ids could not see a fifteenth arrive inert.
WIDGET_IDS=$(awk '/Dashboard Widgets/{f=1} f&&/\{ id: "/{print} f&&/\].filter\(w =>/{exit}' "$DASH" \
             | sed -n 's/.*{ id: "\([a-z_]*\)".*/\1/p')
WN=$(printf '%s\n' "$WIDGET_IDS" | grep -c .)
if [ "$WN" -lt 10 ]; then
  bad "H4 dashboard widget registry" "enumerated only $WN ids — implausible, so H5 would prove nothing"
else
  ok "H4 enumerated $WN dashboard widget ids from the registry itself"
  INERT=""
  for wid in $WIDGET_IDS; do
    case "$F_DASH" in *"isVisible(\"$wid\")"*) ;; *) INERT="$INERT $wid" ;; esac
  done
  [ -z "$INERT" ] && ok "H5 every widget checkbox has a render guard that reads it" \
                  || bad "H5 every widget checkbox has a render guard that reads it" "inert:$INERT"
fi
# The SSL countdown must not go back to riding the Active Issues switch.
# ⚠ COUNTED, not merely present. s442 replaced the original shape — the same
# `isVisible("ssl_countdown")&&...` literal duplicated at an outer whole-row
# gate and an inner per-panel gate — with a single `showSsl` computed once and
# consumed at three independent points (whole-row gate, solo-width detection,
# and the panel's own render gate), so the two copies can no longer drift from
# each other by construction. `eq` on BOTH the definition (still derived from
# ssl_countdown alone, never from "issues") and the total consumer count is
# what makes any one deletion go red — a `has` here is satisfied by whichever
# consumer survives while the others vanish.
eq "H6a the certificate countdown's switch is derived from ssl_countdown alone" \
   "$(occ "$F_DASH" 'constshowSsl=isVisible("ssl_countdown")&&intel.ssl_countdowns.length>0')" 1
eq "H6b that switch is read at all three of its gates (whole-row, solo-width, render)" \
   "$(occ "$F_DASH" 'showSsl')" 4

echo "── §J  the only UNATTENDED renewal carries the site's certificate profile"
# `security_scanner::auto_fix_safe_findings` is reached with no opt-in at all —
# auto-healing is seeded OFF, so this weekly loop is what the product does by
# default on a stock install. Both hand-driven renewal paths attach the site's
# chosen certificate profile; this one selected six columns and never asked for
# it, so the CA quietly issued its DEFAULT and a site the operator had moved off
# the default came back on it — permanently, unattended, and with the column
# still naming the retired choice, which the cooldown and margin helpers go on
# reading. Nothing anywhere reports the downgrade.
#
# ⛔ FUNCTION-SCOPED ON PURPOSE, and the scope IS the arm. Sending the profile is
# what a correct renewal looks like in two OTHER files, so a tree-wide grep is
# satisfied by a sibling that was never broken and the arm cannot fail. Verified
# at s390 by deleting the line from this function alone: J2 goes red while both
# siblings stand.
SCAN=panel/backend/src/services/security_scanner.rs
[ -f "$SCAN" ] || { bad "SETUP" "$SCAN missing"; exit 1; }
F_SCANFN=$(fnbody "$SCAN" 'async fn auto_fix_safe_findings(' | subjin)
n=$(printf '%s' "$F_SCANFN" | wc -c)
[ "$n" -ge 3000 ] || { bad "SETUP" "the scanner's auto-fix body stripped to $n chars, expected >= 3000 — the fnbody anchor has moved and §J is measuring nothing"; exit 1; }
has "J1 the unattended renewal asks the database for the site's profile" \
    "$F_SCANFN" "s.ssl_profile"
has "J2 and sends it with the order, like both hand-driven paths do" \
    "$F_SCANFN" 'agent_body["profile"]=serde_json::json!(profile)'

echo
echo "─────────────────────────────────────────────"
printf 'carry-sweep pins: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
