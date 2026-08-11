#!/usr/bin/env bash
# An actionable refusal must survive the trip to the operator.
#
# The panel's rule (panel/backend/src/error.rs) is deliberate and correct: an
# agent 4xx is an ANSWER and is passed through with its sentence intact, while a
# non-4xx is replaced with "Operation failed. Reference: {uuid}" so an agent
# fault cannot leak internals. Everything this suite pins is the consequence:
# whenever an agent handler answers 5xx for a condition the CALLER can fix, the
# panel destroys the remedy and tells the operator their agent broke — on a
# working agent.
#
# v2.99.0 fixed exactly one instance of this (test_destination's sshpass arm) and
# wrote the rule down in its own comment. A sweep then found the same shape in
# eleven more places, including the unattended nightly backup path fifteen lines
# above the fix. This suite pins each one, and pins BOTH ENDS: a route that maps
# a named constant is a vacuous arm if the service never produces that constant.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

R_FILES=panel/agent/src/routes/files.rs
S_FILES=panel/agent/src/services/files.rs
R_BACKUPS=panel/agent/src/routes/backups.rs
R_VOL=panel/agent/src/routes/volume_backup.rs
S_VOL=panel/agent/src/services/volume_backup.rs
R_STAGING=panel/agent/src/routes/staging.rs
S_STAGING=panel/agent/src/services/staging.rs
R_SEC=panel/agent/src/routes/security.rs
S_SEC=panel/agent/src/services/security.rs
R_SMTP=panel/agent/src/routes/smtp.rs
S_SMTP=panel/agent/src/services/smtp.rs
ERRRS=panel/backend/src/error.rs
for f in "$R_FILES" "$S_FILES" "$R_BACKUPS" "$R_VOL" "$S_VOL" "$R_STAGING" \
         "$S_STAGING" "$R_SEC" "$S_SEC" "$R_SMTP" "$S_SMTP" "$ERRRS"; do
  [ -f "$f" ] || { echo "missing $f"; exit 1; }
done

# Strip // line comments: this suite's whole subject is code whose COMMENTS
# explain the very rule the arms grep for, so without this the prose would
# satisfy the suite. Not stripping /* */ — none of these files use it, and a
# naive block stripper has swallowed a whole file in this repo before.
strip() { sed 's://.*::' "$1"; }
has()  { grep -q  -- "$2" <<< "$1"; }
hasE() { grep -qE -- "$2" <<< "$1"; }
# One function's body, so a sibling handler cannot satisfy an arm.
fnbody() { awk "/(pub )?(async )?fn $2\(/,/^}/" <<< "$1"; }

echo "══ an actionable refusal must survive the trip to the operator ══"
echo
echo "── 0. the panel rule these all depend on ──"

SRC_ERR=$(strip "$ERRRS")
# Asserted in BOTH functions that hold it, separately. A file-wide grep for the
# range is satisfied by either one, so breaking a single copy left it green —
# measured, after `agent_actionable` became the second copy. And the two copies
# drifting apart is precisely the bug this file exists to prevent: one rule for
# a single agent call, a different one for a fleet summary.
for fn in agent_error agent_actionable; do
  BODY=$(fnbody "$SRC_ERR" "$fn")
  if [ "$(printf '%s' "$BODY" | wc -l)" -lt 4 ]; then
    bad "$fn did not extract — the rule it carries is unmeasured"
  elif hasE "$BODY" '\(400\.\.500\)\.contains'; then
    ok "$fn treats a 4xx from the agent as an answer worth passing through"
  else
    bad "$fn's 4xx boundary changed — the arms below defend nothing"
  fi
done

echo
echo "── 1. file manager: policy refusals are not agent faults ──"

SRC_RF=$(strip "$R_FILES"); SRC_SF=$(strip "$S_FILES")

# BOTH ENDS. The service must still emit these sentences, or the route's match
# is dead code and every arm below it is vacuously green.
for sentence in 'File too large (max 2MB)' 'File is binary or not readable as text' \
                'Path already exists' 'Source does not exist' \
                'Destination already exists' 'Path does not exist'; do
  if has "$SRC_SF" "$sentence"; then
    ok "the file service still refuses with \"$sentence\""
  else
    bad "\"$sentence\" is gone from the service — the route arm for it is vacuous"
  fi
done

FS=$(fnbody "$SRC_RF" file_status)
if [ "$(printf '%s' "$FS" | wc -l)" -lt 8 ]; then
  bad "file_status did not extract — its status arms measure nothing"
else
  ok "file_status extracted ($(printf '%s' "$FS" | wc -l) lines)"
  hasE "$FS" 'NOT_FOUND'             && ok "a missing path answers 404"          || bad "a missing path is a 5xx again"
  hasE "$FS" 'CONFLICT'              && ok "a name already taken answers 409"    || bad "a taken name is a 5xx again"
  hasE "$FS" 'PAYLOAD_TOO_LARGE'     && ok "the editor size cap answers 413"     || bad "the size cap is a 5xx again"
  hasE "$FS" 'UNSUPPORTED_MEDIA_TYPE' && ok "a binary file answers 415"          || bad "the binary refusal is a 5xx again"
  hasE "$FS" 'INTERNAL_SERVER_ERROR' && ok "a genuine file fault is still a 5xx" || bad "the classifier is unconditional — real faults now read as user error"
fi

# All four sites must route through it. Counted, because fixing three of four is
# how this class survived in the first place.
SITES=$(grep -c 'map_err(file_status)' "$R_FILES" || true)
if [ "${SITES:-0}" -ge 4 ]; then
  ok "all $SITES file-manager sites classify their refusals"
else
  bad "only ${SITES:-0} file-manager sites classify — read/create/rename/delete must all four"
fi

echo
echo "── 2. restore paths: a missing tool, and an archive that is gone ──"

SRC_RB=$(strip "$R_BACKUPS"); SRC_RV=$(strip "$R_VOL"); SRC_SV=$(strip "$S_VOL")

if hasE "$SRC_RB" 'fn ensure_restic'; then
  ok "the restic presence check is a shared guard"
else
  bad "no shared restic guard — backup and restore will drift again"
fi
RR=$(fnbody "$SRC_RB" restic_restore)
if [ "$(printf '%s' "$RR" | wc -l)" -lt 10 ]; then
  bad "restic_restore did not extract — its guard arms measure nothing"
else
  ok "restic_restore extracted ($(printf '%s' "$RR" | wc -l) lines)"
  hasE "$RR" 'ensure_restic' \
    && ok "restore refuses before running restic, as backup already did" \
    || bad "restore runs restic unguarded again — a rebuilt host gets an incident id mid-recovery"
  hasE "$RR" 'NOT_FOUND' \
    && ok "restoring from a repository that does not exist answers 404" \
    || bad "a missing repository is a 5xx again"
fi

if has "$SRC_SV" 'Backup file not found'; then
  ok "the volume service still refuses a vanished archive by name"
else
  bad "\"Backup file not found\" is gone from the service — the route arm is vacuous"
fi
VR=$(fnbody "$SRC_RV" restore)
if [ "$(printf '%s' "$VR" | wc -l)" -lt 8 ]; then
  bad "the volume restore handler did not extract — its arms measure nothing"
else
  ok "the volume restore handler extracted ($(printf '%s' "$VR" | wc -l) lines)"
  hasE "$VR" 'NOT_FOUND' \
    && ok "an archive the panel listed but the host no longer has answers 404" \
    || bad "a vanished archive is a 5xx again — a retry just mints a new incident id"
fi

echo
echo "── 3. staging: the sentence already named the path ──"

SRC_RS=$(strip "$R_STAGING"); SRC_SS=$(strip "$S_STAGING")
if has "$SRC_SS" 'directory not found'; then
  ok "the staging service still names the directory it could not find"
else
  bad "the staging refusal is gone from the service — the route arm is vacuous"
fi
SY=$(fnbody "$SRC_RS" sync_files)
if [ "$(printf '%s' "$SY" | wc -l)" -lt 8 ]; then
  bad "sync_files did not extract — its arms measure nothing"
else
  ok "sync_files extracted ($(printf '%s' "$SY" | wc -l) lines)"
  hasE "$SY" 'NOT_FOUND' \
    && ok "a missing document root answers 404, keeping the path it named" \
    || bad "the staging refusal is a 5xx again — and it had spelled the exact path"
  hasE "$SY" 'INTERNAL_SERVER_ERROR' \
    && ok "a real sync failure is still a 5xx" \
    || bad "the staging classifier is unconditional"
fi

echo
echo "── 4. firewall + mail: a package the panel will never install there ──"

SRC_SEC=$(strip "$R_SEC"); SRC_SSEC=$(strip "$S_SEC")
SRC_RSM=$(strip "$R_SMTP"); SRC_SSM=$(strip "$S_SMTP")

hasE "$SRC_SSEC" 'pub const UFW_MISSING' \
  && ok "the ufw refusal is a named constant, not a sentence to guess at" \
  || bad "UFW_MISSING is gone — the route is back to substring-matching prose"
# The constant must be what the service actually returns, or the route's
# equality test never fires. This is the end that was never checked.
USES=$(grep -c 'UFW_MISSING.to_string()' "$S_SEC" || true)
if [ "${USES:-0}" -ge 2 ]; then
  ok "both ufw call paths raise the named constant ($USES)"
else
  bad "only ${USES:-0} ufw path(s) raise the constant — add and delete must both"
fi

SS=$(fnbody "$SRC_SEC" security_status)
if [ "$(printf '%s' "$SS" | wc -l)" -lt 8 ]; then
  bad "security_status did not extract — its arms measure nothing"
else
  ok "security_status extracted ($(printf '%s' "$SS" | wc -l) lines)"
  hasE "$SS" 'UFW_MISSING' \
    && ok "the classifier keys on the constant rather than on wording" \
    || bad "the classifier guesses at the sentence again — the way this defect was born"
  hasE "$SS" 'FAILED_DEPENDENCY' \
    && ok "a missing firewall package answers 4xx, so its sentence survives" \
    || bad "the ufw refusal is a 5xx again — every RHEL box loses the reason"
  hasE "$SS" 'INTERNAL_SERVER_ERROR' \
    && ok "a genuine firewall fault is still a 5xx" \
    || bad "the security classifier is unconditional"
fi
SEC_SITES=$(grep -c 'map_err(security_status)' "$R_SEC" || true)
if [ "${SEC_SITES:-0}" -ge 3 ]; then
  ok "all $SEC_SITES security handlers classify (add, delete, apply-fix)"
else
  bad "only ${SEC_SITES:-0} security handler(s) classify — add/delete/apply-fix must all three"
fi

hasE "$SRC_SSM" 'pub const MSMTP_MISSING' \
  && ok "the msmtp refusal is a named constant" \
  || bad "MSMTP_MISSING is gone — Test Email is back to blaming the operator's relay"
has "$SRC_SSM" 'MSMTP_MISSING.to_string()' \
  && ok "the mail service raises it when the binary is absent" \
  || bad "the service never raises MSMTP_MISSING — the route arm is vacuous"
TE=$(fnbody "$SRC_RSM" test_email)
if [ "$(printf '%s' "$TE" | wc -l)" -lt 8 ]; then
  bad "test_email did not extract — its arms measure nothing"
else
  ok "test_email extracted ($(printf '%s' "$TE" | wc -l) lines)"
  hasE "$TE" 'FAILED_DEPENDENCY' \
    && ok "a host with no msmtp answers 4xx instead of implicating the relay" \
    || bad "Test Email is a 5xx again — the operator will go debug their mail provider"
  hasE "$TE" 'INTERNAL_SERVER_ERROR' \
    && ok "a genuine send failure is still a 5xx" \
    || bad "the mail classifier is unconditional"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
