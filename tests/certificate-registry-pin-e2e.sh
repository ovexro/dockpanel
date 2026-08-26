#!/usr/bin/env bash
# certificate-registry-pin-e2e.sh — s408 / v2.160.0 (GitHub #104)
#
# Pins the named certificate registry and the STORED TLS mode on a stack —
# the three points the reporter and the panel settled in writing, plus the
# constraint the panel found on its own:
#
#   §B  the mode is STORED and honoured on every redeploy. Every one of the
#       three bodies the panel sends the agent carries it; an edit that says
#       nothing about TLS keeps what the row says, domain included. Before this
#       an edit forwarded the request's address verbatim, so omitting it took
#       the certificate off a stack that had one — behind a year of HSTS.
#   §C  a provided certificate is refused on an agent too old to honour the
#       mode, fail-closed, by a FROZEN constant read off the agent's own answer.
#   §D  the registry's lifecycle on the panel: uniqueness asked before the
#       agent writes, delete refused while a stack still names the alias, and
#       a pair already gone from the disk not making its row immortal.
#   §E  the migration: a CHECK on the vocabulary, SET NULL and never RESTRICT,
#       the ssl_email-derived backfill, alias unique per server.
#   §F  the agent: the provided arm runs BEFORE the HTTP-first write and never
#       through the per-domain enabler; a missing or unfit certificate leaves
#       the vhost untouched; the registry root is a sibling of the per-domain
#       tree, not inside it; nothing is written before every check passes.
#   §G  the seams: one mode vocabulary across migration, backend, agent and
#       page; the link scheme no longer keyed on the address; the SPA refreshes
#       after the kept-row refusal; the agent's refusal FLAG is a boolean and
#       the panel reads the SENTENCE from the warning beside it.
#
# Pure source analysis: no box, no network, no build.
#
# ⚠ EVERY arm reads STRIPPED source. Both trees carry long comments naming the
# literals below — the mode words, the flag, the registry root. Against raw
# text half of §F and §G would be satisfied by the prose describing them.
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
eq()  { [ "$2" = "$3" ] && ok "$1" || bad "$1 — expected '$3', got '$2'"; }
has() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 — missing: $3" ;; esac; }
hasnt() { case "$2" in *"$3"*) bad "$1 — present but must not be: $3" ;; *) ok "$1" ;; esac; }

STACKS=panel/backend/src/routes/stacks.rs
TLSC=panel/backend/src/routes/tls_certificates.rs
BMOD=panel/backend/src/routes/mod.rs
MIG=panel/backend/migrations/20260826000000_tls_certificate_registry.sql
ADA=panel/agent/src/routes/docker_apps.rs
AREG=panel/agent/src/routes/ssl_registry.rs
ASSL=panel/agent/src/services/ssl.rs
AOWN=panel/agent/src/services/ownership.rs
AMAIN=panel/agent/src/main.rs
APPS=panel/frontend/src/pages/Apps.tsx
CERTS=panel/frontend/src/pages/Certificates.tsx
REG=panel/frontend/src/components/RegisteredCertificates.tsx

for f in "$STACKS" "$TLSC" "$BMOD" "$MIG" "$ADA" "$AREG" "$ASSL" "$AOWN" "$AMAIN" "$APPS" "$CERTS" "$REG"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

# Comments out, CODE INTACT. Line comments first, then block comments, so a
# `/*` inside a line comment cannot swallow the file.
code() {
  perl -0777 -pe '
    s{\{/\*.*?\*/\}}{}gs;
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*--.*$}{}gm;
  ' "$1"
}
subj() { local t; t=$(code "$1"); [ -n "$t" ] || return 1; printf '%s' "$t"; }
# Whitespace and line-continuation backslashes removed, so a literal matches
# however rustfmt or prettier wrapped it.
flat() { printf '%s' "$1" | tr -d ' \n\\'; }
# Byte offset of the FIRST occurrence of $2 in $1, or -1.
pos() { local t=${1%%"$2"*}; [ "$t" = "$1" ] && echo -1 || echo "${#t}"; }
# Occurrences of the fixed string $2 in $1.
occ() { printf '%s' "$1" | /usr/bin/grep -oF -- "$2" | wc -l | tr -d ' '; }

# The body of one top-level Rust fn, bounded by the NEXT top-level fn or impl.
fnbody() {
  awk -v name="$2" '
    /^[[:space:]]*(pub )?(pub\(crate\) )?(async )?fn / || /^impl / || /^#\[cfg\(test\)\]/ {
      if ($0 ~ "(pub |pub\\(crate\\) )?(async )?fn " name "\\(") { inside=1; next }
      inside=0
    }
    inside { print }
  ' <<< "$1"
}
# A body that must be at least $3 lines, or the arm cannot be trusted.
body() {
  local b; b=$(fnbody "$1" "$2"); local n; n=$(wc -l <<< "$b")
  if [ "$n" -ge "$3" ]; then ok "A-body $2 ($n lines)"; else bad "A-body $2 — extracted as $n lines, subject lost"; fi
  printf '%s' "$b"
}
# Both offsets found, and the first before the second.
before() { local a b; a=$(pos "$2" "$3"); b=$(pos "$2" "$4"); if [ "$a" -ge 0 ] && [ "$b" -ge 0 ] && [ "$a" -lt "$b" ]; then ok "$1"; else bad "$1 — offsets $a / $b"; fi; }

S=$(subj "$STACKS") || { bad "stacks.rs stripped to nothing"; S=""; }
T=$(subj "$TLSC") || { bad "tls_certificates.rs stripped to nothing"; T=""; }
BM=$(subj "$BMOD") || { bad "backend routes/mod.rs stripped to nothing"; BM=""; }
MG=$(subj "$MIG") || { bad "migration stripped to nothing"; MG=""; }
D=$(subj "$ADA") || { bad "agent docker_apps.rs stripped to nothing"; D=""; }
R=$(subj "$AREG") || { bad "ssl_registry.rs stripped to nothing"; R=""; }
L=$(subj "$ASSL") || { bad "agent ssl.rs stripped to nothing"; L=""; }
OWN=$(subj "$AOWN") || { bad "ownership.rs stripped to nothing"; OWN=""; }
AM=$(subj "$AMAIN") || { bad "agent main.rs stripped to nothing"; AM=""; }
AP=$(subj "$APPS") || { bad "Apps.tsx stripped to nothing"; AP=""; }
CT=$(subj "$CERTS") || { bad "Certificates.tsx stripped to nothing"; CT=""; }
RC=$(subj "$REG") || { bad "RegisteredCertificates.tsx stripped to nothing"; RC=""; }

echo "== §A  controls: every subject is floored, and the number is printed =="
for pair in "stacks.rs:$S:900" "tls_certificates.rs:$T:400" "routes/mod.rs:$BM:900" "migration:$MG:20" \
            "agent docker_apps.rs:$D:1500" "ssl_registry.rs:$R:250" "agent ssl.rs:$L:900" \
            "ownership.rs:$OWN:250" "agent main.rs:$AM:150" "Apps.tsx:$AP:2000" \
            "Certificates.tsx:$CT:200" "RegisteredCertificates.tsx:$RC:250"; do
  name=${pair%%:*}; rest=${pair#*:}; min=${rest##*:}; text=${rest%:*}
  n=$(wc -l <<< "$text")
  if [ "$n" -ge "$min" ]; then ok "A-control $name stripped subject is $n lines"; else bad "A-control $name stripped subject is only $n lines — subject lost"; fi
done

echo "== §B  the mode is stored and honoured on every redeploy =="
FS=$(flat "$S")
has "B1 STACK_SELECT projects the stored mode" "$FS" 's.tls_mode,s.tls_certificate_id,c.aliasAStls_certificate'
eq  "B2-control stacks.rs sends the agent exactly three compose deploys" "$(occ "$S" '"/apps/compose/deploy"')" "3"
CRE=$(body "$S" create 120); UPD=$(body "$S" update 180); RES=$(body "$S" restore_previous 25)
# Scoped to the DEPLOY BODY of each fn — the create/update responses in the
# same bodies spell the same keys, and a whole-body `has` was satisfied by them
# with the deploy bodies stripped (caught by mutation, s408).
for pair in "create:$CRE" "update:$UPD" "restore_previous:$RES"; do
  fn=${pair%%:*}; b=$(flat "${pair#*:}")
  db=${b#*\"/apps/compose/deploy\",Some(serde_json::json!({}; db=${db%%\})\)*}
  eq "B2-control $fn's deploy body was sliced" "$([ "${#db}" -gt 60 ] && [ "${#db}" -lt 600 ] && echo sized || echo "${#db}")" "sized"
  has "B2 $fn's deploy body carries the mode" "$db" '"tls_mode":'
  has "B2 $fn's deploy body carries the alias" "$db" '"tls_certificate":'
done
FCRE=$(flat "$CRE"); FUPD=$(flat "$UPD"); FRES=$(flat "$RES")
has "B3 create sends the address only for an ACME order" "$FCRE" '"ssl_email":tls.deploy_email(),'
has "B3 update sends the address only for an ACME order" "$FUPD" '"ssl_email":tls.deploy_email(),'
has "B3 restore_previous sends the address only for an ACME order" "$FRES" '"ssl_email":iftls_mode=="acme"{ssl_email}else{None},'
has "B4 update reads the stored mode off the row" "$FUPD" 'effective_tls_mode(previous_tls_mode.as_deref(),previous_ssl_email.as_deref())'
has "B4 update keeps the stored mode when the request names none" "$FUPD" '(None,true)=>previous_mode,'
has "B4 update keeps the stored domain when the request omits it" "$FUPD" 'matchbody.domain{None=>previous_domain.clone(),'
has "B4 a vacated domain resolves to no mode" "$FUPD" '(None,false)=>"none",'
has "B4 update keeps the stored address when the request omits it" "$FUPD" '.or(previous_ssl_email.clone())'
has "B4 update keeps the stored alias when the request omits it" "$FUPD" '.or(previous_alias.clone())'
has "B5 the update request tells absent from null for the domain" "$FS" 'deserialize_with="super::secrets::explicit_option")]pubdomain:Option<Option<String>>,'
PLAN=$(body "$S" plan_tls 40); FPLAN=$(flat "$PLAN")
before "B6 plan_tls resolves the alias before gating the agent" "$FPLAN" 'certificate_id_for_alias(' 'require_agent_at_least('
before "B6 plan_tls gates the agent before asking it about coverage" "$FPLAN" 'require_agent_at_least(' '/covers'
has "B6 the coverage question is the agent's, not a copy in the panel" "$FPLAN" '"/ssl/registry/{alias}/covers"'
before "B7 create plans TLS before writing the row" "$FCRE" 'plan_tls(' 'INSERTINTOdocker_stacks'
before "B7 update plans TLS before tearing anything down" "$FUPD" 'plan_tls(' '"action":"remove"'
REF=$(body "$S" provided_tls_refusal 8); FREF=$(flat "$REF")
has "B8 a provided deploy succeeds only on an explicit ssl:true" "$FREF" 'get("ssl").and_then(|v|v.as_bool())==Some(true)'
before "B8 the sentence is read from the warning, the flag is only a flag" "$FREF" 'get("proxy_warning")' 'get("tls_refused")'
has "B8 create turns a refused provided deploy into an error" "$FCRE" 'provided_tls_refusal(&deploy_result)'
has "B8 update turns a refused provided deploy into an error" "$FUPD" 'provided_tls_refusal(&deploy_result)'
EFF=$(body "$S" effective_tls_mode 8); FEFF=$(flat "$EFF")
has "B9 the stored value wins" "$FEFF" 'ifletSome(stored)=tls_mode{'
has "B9 a NULL row derives the mode from the address, as the agent always did" "$FEFF" 'ifhas_email{"acme"}else{"none"}'

echo "== §C  an agent too old to honour the mode is refused, fail-closed =="
FT=$(flat "$T")
eq  "C1 the minimum agent is a FROZEN literal — a release bump must not move it" "$(occ "$FT" 'PROVIDED_TLS_MIN_AGENT:&str="2.160.0";')" "1"
GATE=$(body "$T" require_agent_at_least 10); FGATE=$(flat "$GATE")
has "C2 the gate reads the agent's own answer, not a column that is NULL on the local box" "$FGATE" 'agent.get("/health")'
has "C2 the gate compares releases, not strings" "$FGATE" 'panel_update::semver_key'
has "C2 a too-old agent is a precondition failure the operator can act on" "$FGATE" 'PRECONDITION_FAILED'
has "C3 a provided claim goes through the gate" "$FPLAN" 'PROVIDED_TLS_MIN_AGENT'
TCRE=$(body "$T" create 40); FTCRE=$(flat "$TCRE")
before "C3 registering a certificate goes through the gate before the agent is asked to write" "$FTCRE" 'require_agent_at_least(' '"/ssl/registry"'

echo "== §D  the registry's lifecycle on the panel =="
before "D1 alias uniqueness is asked BEFORE the agent writes anything" "$FTCRE" 'WHEREserver_id=$1ANDalias=$2' '"/ssl/registry"'
TREM=$(body "$T" remove 25); FTREM=$(flat "$TREM")
before "D2 delete asks which stacks still name the alias before touching the agent" "$FTREM" 'referencing_stacks(' '/ssl/registry/'
before "D2 the agent's copy goes before the row" "$FTREM" '/ssl/registry/' 'DELETEFROMtls_certificates'
has "D2 a referenced certificate is refused with a conflict" "$FTREM" 'StatusCode::CONFLICT'
has "D2 a pair already gone from the disk does not make its row immortal" "$FTREM" 'AgentError::Status(404,_)'
TREP=$(body "$T" replace 25); FTREP=$(flat "$TREP")
has "D3 a replacement must still cover every domain served under the alias" "$FTREP" '"must_cover":'
has "D3 a replacement says so to the agent" "$FTREP" '"replace":true,'
FBM=$(flat "$BM")
eq "D4 the collection route is registered once, in the router" "$(occ "$FBM" '"/api/tls-certificates",get(tls_certificates::list).post(tls_certificates::create)')" "1"
eq "D4 the item route is registered once, in the router" "$(occ "$FBM" '"/api/tls-certificates/{id}",put(tls_certificates::replace).delete(tls_certificates::remove)')" "1"
eq "D4 the module registers no route of its own" "$(occ "$T" '.route(')" "0"

echo "== §E  the migration =="
FMG=$(flat "$MG")
has "E1 the mode column carries the vocabulary as a CHECK" "$FMG" "tls_modeTEXTCHECK(tls_modeIN('none','acme','provided'))"
has "E2 a deleted certificate nulls the reference" "$FMG" 'tls_certificate_idUUIDREFERENCEStls_certificates(id)ONDELETESETNULL'
hasnt "E3 no RESTRICT — a blocking key is a boot that cannot happen" "$FMG" 'RESTRICT'
has "E4 the backfill derives the mode from the only signal that existed" "$FMG" "SETtls_mode=CASEWHENssl_emailISNOTNULLANDssl_email<>''THEN'acme'ELSE'none'END"
has "E4 …and only where nothing is recorded yet" "$FMG" 'WHEREtls_modeISNULL'
has "E5 an alias is unique per server" "$FMG" 'UNIQUE(server_id,alias)'
eq "E6 this migration is the newest on disk" "$(ls panel/backend/migrations/*.sql | sort | tail -1)" "$MIG"

echo "== §F  the agent honours the mode and keeps the registry apart =="
FD=$(flat "$D")
eq "F1 both deploy requests carry the mode, defaulted for an older panel" "$(occ "$FD" '#[serde(default)]tls_mode:Option<String>,')" "2"
eq "F1 both deploy requests carry the alias, defaulted for an older panel" "$(occ "$FD" '#[serde(default)]tls_certificate:Option<String>,')" "2"
FRQ=$(body "$D" from_request 20); FFRQ=$(flat "$FRQ")
has "F2 an older panel's request keeps today's presence rule byte-for-byte" "$FFRQ" 'None=>Ok(matchssl_email{Some(email)=>TlsIntent::Acme{email},None=>TlsIntent::None,}),'
has "F2 a provided intent needs a well-formed alias" "$FFRQ" 'Some("provided")=>matchalias{Some(alias)ifssl::is_valid_cert_alias(alias)=>Ok(TlsIntent::Provided{alias}),'
EXP=$(body "$D" expose_domain 150); FEXP=$(flat "$EXP")
before "F3 the provided arm runs BEFORE the HTTP-first write" "$FEXP" 'ssl::registry_paths(alias)' 'letsite_config=proxy_site_config(port);'
eq "F4 the per-domain enabler is called once — by the ACME arm" "$(occ "$FEXP" 'enable_ssl_for_site(')" "1"
before "F4 …and only after the HTTP-first write, so the provided arm never reaches it" "$FEXP" 'letsite_config=proxy_site_config(port);' 'enable_ssl_for_site('
before "F5 the provided arm re-asks coverage before it writes" "$FEXP" 'ssl::cert_covers_domain(&pem,domain)' 'std::fs::write(&tmp_path'
before "F6 a missing certificate refuses before anything is written" "$FEXP" '"tls_refused"' 'std::fs::write(&tmp_path'
# The provided slice: from the registry lookup to the HTTP-first write.
PROV=${FEXP#*ssl::registry_paths(alias)}; PROV=${PROV%%letsite_config=proxy_site_config(port);*}
eq "F7-control the provided slice is non-empty" "$([ "${#PROV}" -gt 800 ] && echo big || echo "${#PROV}")" "big"
eq "F7 three refusals inside the arm: missing, unfit, not reloaded" "$(occ "$PROV" '"tls_refused"]=serde_json::json!(true)')" "3"
eq "F7 the flag is a boolean everywhere the agent sets it" "$(occ "$FD" '"tls_refused"]=serde_json::json!(true)')" "4"
eq "F7 …and never the sentence" "$(occ "$FD" '"tls_refused"]=serde_json::json!(format')" "0"
# The parked branch binds the certificate before the reload ever runs, so the
# order is asked of the LIVE branch: from the reload onward, the refusal flag
# must come before the success key.
LIVE=${PROV#*nginx::reload().await{}
eq "F8-control the live branch was sliced" "$([ "${#LIVE}" -gt 200 ] && echo big || echo "${#LIVE}")" "big"
before "F8 a reload that fails is a refusal, not a success" "$LIVE" '"tls_refused"' 'response["ssl"]=serde_json::json!(true)'
eq "F9 the parked write and the live write both report the certificate bound" "$(occ "$PROV" 'response["ssl"]=serde_json::json!(true)')" "2"
has "F9 a parked write says it is parked" "$PROV" 'response["parked"]=serde_json::json!(true)'
FL=$(flat "$L")
ROOT=$(printf '%s' "$FL" | /usr/bin/grep -oE 'SSL_REGISTRY_DIR:&str="[^"]+"' | head -1 | sed 's/.*="//; s/"$//')
eq "F10-control the registry root was extracted" "$([ -n "$ROOT" ] && echo yes || echo no)" "yes"
case "$ROOT" in /etc/dockpanel/ssl/*|/etc/dockpanel/ssl) bad "F10 the registry root sits INSIDE the per-domain tree ($ROOT) — every walker would call an alias a domain";; /etc/dockpanel/*) ok "F10 the registry root is a sibling under the agent's writable tree ($ROOT)";; *) bad "F10 the registry root is outside the agent's writable tree ($ROOT)";; esac
RP=$(body "$L" registry_paths 3); has "F10 registry paths are built from the root, never the per-domain tree" "$(flat "$RP")" 'SSL_REGISTRY_DIR'
FR=$(flat "$R")
REGB=$(body "$R" register 60); FREGB=$(flat "$REGB")
before "F11 the certificate is parsed before the key is matched" "$FREGB" 'cert_metadata(' 'key_matches_cert('
before "F11 the key is matched before coverage is asked" "$FREGB" 'key_matches_cert(' 'cert_covers_domain('
before "F11 coverage is asked before anything is staged" "$FREGB" 'cert_covers_domain(' 'stage_pair('
before "F11 a taken alias is refused before anything is staged" "$FREGB" 'StatusCode::CONFLICT' 'stage_pair('
RREM=$(body "$R" remove 20); FRREM=$(flat "$RREM")
before "F12 delete asks which vhosts still name the pair before removing it" "$FRREM" 'registry_cert_references(' 'remove_dir_all(&final_dir)'
has "F12 an absent alias is told apart from an unreadable one" "$FRREM" 'ErrorKind::NotFound'
eq "F13 the registry serves exactly three routes" "$(occ "$R" '.route(')" "3"
has "F13 …the upload" "$FR" '.route("/ssl/registry",post(register))'
has "F13 …the coverage question" "$FR" '.route("/ssl/registry/{alias}/covers",post(covers))'
has "F13 …the delete" "$FR" '.route("/ssl/registry/{alias}",delete(remove))'
eq "F13 the router is merged once, before the auth layer" "$(occ "$(flat "$AM")" '.merge(routes::ssl_registry::router())')" "1"
KM=$(body "$L" key_matches_cert 10); FKM=$(flat "$KM")
has "F14 the key is matched the way rustls would before serving the pair" "$FKM" '.keys_match()'
has "F14 …through the provider the binary already installs" "$FKM" 'any_supported_type('
UNX=$(body "$D" unexpose_domain 20)
hasnt "F15 the stack teardown never reaches the registry" "$(flat "$UNX")" 'SSL_REGISTRY_DIR'
for f in "$ASSL" "$AREG" "$ADA"; do
  first=$(/usr/bin/grep -n '^#\[cfg(test)\]' "$f" | head -1 | cut -d: -f1)
  if [ -z "$first" ]; then ok "F16 $(basename "$f") has no test module to hide production code below"; continue; fi
  below=$(awk -v s="$first" 'NR>s && /^(pub |async fn |fn |impl |struct |const |static )/' "$f" | wc -l)
  eq "F16 no production item sits below the first test module in $(basename "$f")" "$below" "0"
done

echo "== §G  the seams =="
# One vocabulary, DERIVED from each tree — a word that drifts in one place
# turns the comparison red, and the character class admits an underscore.
V_MIG=$(printf '%s' "$FMG" | /usr/bin/grep -oE "tls_modeIN\([^)]*\)" | /usr/bin/grep -oE "'[a-z_]+'" | tr -d "'" | sort -u | tr '\n' ' ')
V_BE=$(printf '%s' "$FS" | /usr/bin/grep -oE 'constTLS_MODES:\[&str;[0-9]+\]=\[[^]]*\]' | /usr/bin/grep -oE '"[a-z_]+"' | tr -d '"' | sort -u | tr '\n' ' ')
V_AG=$(printf '%s' "$FFRQ" | /usr/bin/grep -oE 'Some\("[a-z_]+"\)=>' | /usr/bin/grep -oE '"[a-z_]+"' | tr -d '"' | sort -u | tr '\n' ' ')
SEL=${AP#*id=\"compose-tls-mode\"}; SEL=${SEL%%</select>*}
V_FE=$(printf '%s' "$SEL" | /usr/bin/grep -oE '<option value="[a-z_]+"' | /usr/bin/grep -oE '"[a-z_]+"' | tr -d '"' | sort -u | tr '\n' ' ')
eq "G1-control the migration's vocabulary was extracted" "$(wc -w <<< "$V_MIG" | tr -d ' ')" "3"
eq "G1 backend and migration agree on the vocabulary" "$V_BE" "$V_MIG"
eq "G1 agent and migration agree on the vocabulary" "$V_AG" "$V_MIG"
eq "G1 the page offers exactly the vocabulary" "$V_FE" "$V_MIG"
FAP=$(flat "$AP")
hasnt "G2 the stack link no longer infers its scheme from the address" "$FAP" 'stack.ssl_email?"https"'
has "G2 the stack link reads the stored mode" "$FAP" 'stack.tls_mode!=="none"?"https":"http"'
has "G3 the mode is sent as a plain word" "$FAP" 'tls_mode:composeTlsMode,'
has "G3 the alias is sent only in provided mode" "$FAP" 'composeTlsMode==="provided"?{tls_certificate:composeTlsCertificate}:{}'
hasnt "G3 the new fields do not take the null-for-empty spelling a sibling pin counts" "$FAP" 'tls_mode:composeTlsMode||null'
DEP=${FAP#*consthandleComposeDeploy=async()=>{}; DEP=${DEP%%consthandleStackAction=*}
eq "G4-control the deploy handler was sliced" "$([ "${#DEP}" -gt 400 ] && echo big || echo "${#DEP}")" "big"
has "G4 a refusal after the stack exists still refreshes the list" "$DEP" 'catch(e){loadApps();'
FCT=$(flat "$CT"); FRC=$(flat "$RC")
has "G5 the page exports its status vocabulary for the registry section" "$FCT" 'exportconstSTATUS_STYLES:Record<string,{bg:string;text:string;label:string}>={'
has "G5 the registry section reuses it rather than a second map" "$FRC" 'import{STATUS_STYLES}from"../pages/Certificates";'
has "G5 …for every row, falling back to unknown" "$FRC" 'STATUS_STYLES[row.status]||STATUS_STYLES.unknown'
eq "G5 the section is rendered once, admin-gated" "$(occ "$FCT" '{isAdmin&&<RegisteredCertificates/>}')" "1"
has "G6 the section lists the registry" "$FRC" 'api.get<RegisteredCertificate[]>("/tls-certificates")'
has "G6 …registers through it" "$FRC" 'api.post("/tls-certificates",{alias:name,certificate:certPem,private_key:keyPem})'
has "G6 …replaces through it" "$FRC" 'api.put(`/tls-certificates/${row.id}`,{certificate:replaceCert,private_key:replaceKey})'
has "G6 …and deletes through it" "$FRC" 'api.delete(`/tls-certificates/${row.id}`)'
has "G7 the page checks the alias grammar the agent enforces" "$FRC" 'ALIAS_RE=/^[a-z0-9]([a-z0-9-]{0,62}[a-z0-9])?$/;'
has "G7 the agent's grammar caps at 64" "$(flat "$(body "$L" is_valid_cert_alias 4)")" '>64'
has "G7 the panel's grammar caps at 64" "$(flat "$(body "$T" is_valid_cert_alias 4)")" '>64'

echo
printf 'PASS %d  FAIL %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
