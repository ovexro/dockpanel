#!/usr/bin/env bash
# scan-alert-resolve-pin-e2e.sh — a scan that comes back clean must clear the
# alert an earlier dirty scan raised, and a dirty image scan must page at all
#
#   §A  trigger_scan (manual "Run Scan") resolves stale firing/acknowledged
#       security alerts BEFORE deciding whether to raise a new one — its
#       scheduled twin, security_scanner::run_scan, always did this; the
#       manual path never did, so clicking "Run Scan" to confirm a fix left
#       the dashboard's firing-alert badge and its -15 health-score penalty
#       stuck until the next weekly scheduled scan, up to 7 days later, even
#       though the scan-history tiles (read from security_scans directly)
#       updated instantly. Found by dockpanel-fanout's completeness critic
#       (s422), not by any of its four assigned topics.
#   §B  image_scans::scan_and_store fires an alert on a critical/high finding
#       and resolves it once the image scans clean again. Before this, NEITHER
#       the manual/deploy-triggered scan NOR the 30-minute background sweep
#       (which calls this same function against every fleet member) ever
#       raised an alert for a discovered vulnerability — a critical CVE in a
#       running container produced a row in image_scan_findings and nothing
#       else: no page, no firing count, no bell. Found by dockpanel-fanout's
#       convention-drift topic (s422).
#   §C  wordpress::scan_and_store (s472) is image_scan's own direct peer for
#       WordPress plugin CVEs — same resolve-then-fire shape, but PER-SITE-
#       OWNER rather than per-admin, since a WordPress site belongs to one
#       user and an image is a shared server-wide resource. Before this,
#       `wordpress::vuln_scan` (the manual "Scan" button) stored a scan and
#       raised NOTHING — no schedule, no page, no bell — the exact §B gap,
#       just never closed for this scanner. wp_vuln_scanner.rs is the new
#       30-minute background sweep, the WP-toolkit twin of image_scanner.rs.
#
# Pure source analysis: no box, no network, no build.
#
# NO PIPES INTO `grep -q` — under `set -o pipefail` grep -q closes the pipe on
# its first match and the arm goes red on correct code. Every arm uses a
# here-string.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

SEC_SCANS=panel/backend/src/routes/security_scans.rs
IMG_SCANS=panel/backend/src/routes/image_scans.rs
WP=panel/backend/src/routes/wordpress.rs
WP_SCANNER=panel/backend/src/services/wp_vuln_scanner.rs

for f in "$SEC_SCANS" "$IMG_SCANS" "$WP" "$WP_SCANNER"; do
  [ -f "$f" ] || bad "MISSING SUBJECT FILE: $f"
done

echo "== §A  a manual security rescan clears the alert its own fix disproved =="

# A1-control: body extraction. Bounded to the function, not the whole file —
# security_scans.rs holds three other handlers.
TRIGGER=$(awk '/^pub async fn trigger_scan\(/{i=1} i{print} i && /^}$/{exit}' "$SEC_SCANS")
NTRIG=$(grep -c . <<< "$TRIGGER")
if [ "$NTRIG" -ge 60 ]; then
  ok "A1-control trigger_scan body extracted — $NTRIG lines"
else
  bad "A1-control trigger_scan body extracted — only $NTRIG lines (the extractor broke)"
fi

# A2: the resolve call exists in the handler at all.
if grep -qE 'notifications::resolve_alert\(' <<< "$TRIGGER"; then
  ok "A2 trigger_scan calls resolve_alert"
else
  bad "A2 trigger_scan calls resolve_alert — manual rescan cannot clear a stale firing alert"
fi

# A2b: FAN OUT TO EVERY ADMIN, not just the clicking user. `run_scan` (the
# scheduled twin this mirrors) fires `security` alerts per-admin
# (`send_scan_alerts`), so a multi-admin install can have one firing row PER
# ADMIN for the same condition. Resolving only `claims.sub` — the admin who
# happened to click "Run Scan" — would leave every OTHER admin's row stuck,
# reproducing the exact bug this fix exists to close for everyone but the
# clicker. Caught by adversarial review (s422) before this ever shipped.
if grep -qE "SELECT id FROM users WHERE role = 'admin'" <<< "$TRIGGER" \
   && grep -qE 'for \(user_id,\) in &admins' <<< "$TRIGGER"; then
  ok "A2b the resolve fans out to every admin, not just the one who clicked"
else
  bad "A2b the resolve must fan out to every admin — a single-user resolve leaves other admins' rows stuck on a multi-admin install"
fi

# A3: POSITIONAL, not presence — matching the alert-controls suite's own G1
# style. A resolve call placed AFTER the fire decision (or inside the dirty
# branch only) would still pass a presence check while leaving a clean scan
# with no resolve at all, which is the exact defect this pin exists to catch.
RESOLVE_LINE=$(grep -n 'notifications::resolve_alert(' <<< "$TRIGGER" | head -1 | cut -d: -f1)
FIRE_LINE=$(grep -n 'if critical > 0 || warning > 0' <<< "$TRIGGER" | head -1 | cut -d: -f1)
if [ -n "$RESOLVE_LINE" ] && [ -n "$FIRE_LINE" ] && [ "$RESOLVE_LINE" -lt "$FIRE_LINE" ]; then
  ok "A3 the resolve (line $RESOLVE_LINE) runs unconditionally BEFORE the dirty-branch fire decision (line $FIRE_LINE)"
else
  bad "A3 the resolve must precede the fire decision — resolve=${RESOLVE_LINE:-none} fire=${FIRE_LINE:-none}"
fi

# A4: the resolve targets the 'security' alert_type with server_id scoping —
# not a bare call that happens to exist for an unrelated type.
RESOLVE_CALL=$(perl -0777 -ne '
  while (/notifications::resolve_alert\s*(\((?:[^()]++|(?1))*\))/gs) {
    my $c = $1; $c =~ s/\s+/ /g; print "$c\n";
  }
' <<< "$TRIGGER")
if grep -qE 'Some\(server_id\)' <<< "$RESOLVE_CALL" && grep -qE '"security"' <<< "$RESOLVE_CALL"; then
  ok "A4 the resolve is scoped to this server's 'security' alerts"
else
  bad "A4 the resolve is scoped to this server's 'security' alerts — call was: $RESOLVE_CALL"
fi

# A5: the acknowledged-row UPDATE also exists — resolve_alert only clears
# status='firing'; an operator who had already acknowledged the earlier alert
# needs the row moved to 'resolved' too, mirroring run_scan's own direct UPDATE.
ACK_BLOCK=$(sed -n "/${RESOLVE_LINE:-9999},/,/if critical > 0/p" <<< "$TRIGGER" 2>/dev/null)
if grep -qE "UPDATE alerts SET status = 'resolved'" <<< "$TRIGGER" \
   && grep -qE "status = 'acknowledged'" <<< "$TRIGGER"; then
  ok "A5 an acknowledged security alert for this server is also cleared"
else
  bad "A5 an acknowledged security alert for this server is also cleared"
fi

# A6-control / A7: run_scan (the scheduled twin) still does the same thing —
# a positive control proving A2-A5 aren't measuring a pattern that was deleted
# from both sides at once.
SCANNER=panel/backend/src/services/security_scanner.rs
RUNSCAN=$(awk '/^async fn run_scan\(/{i=1} i{print} i && /^}$/{exit}' "$SCANNER" 2>/dev/null)
NRUN=$(grep -c . <<< "$RUNSCAN")
if [ "$NRUN" -ge 60 ]; then
  ok "A6-control run_scan body extracted — $NRUN lines"
else
  bad "A6-control run_scan body extracted — only $NRUN lines (the extractor broke)"
fi
if grep -qE 'notifications::resolve_alert\(' <<< "$RUNSCAN"; then
  ok "A7 the scheduled twin (run_scan) still resolves too — the two paths are symmetric again"
else
  bad "A7 the scheduled twin (run_scan) still resolves — control failed, A2-A5 may be vacuous"
fi

echo
echo "== §B  an image vulnerability scan pages an admin, and clears when fixed =="

# B1-control: body extraction.
STORE=$(awk '/^pub async fn scan_and_store\(/{i=1} i{print} i && /^}$/{exit}' "$IMG_SCANS")
NSTORE=$(grep -c . <<< "$STORE")
if [ "$NSTORE" -ge 60 ]; then
  ok "B1-control scan_and_store body extracted — $NSTORE lines"
else
  bad "B1-control scan_and_store body extracted — only $NSTORE lines (the extractor broke)"
fi

# B2: both calls exist.
if grep -qE 'notifications::resolve_alert\(' <<< "$STORE"; then
  ok "B2a scan_and_store calls resolve_alert"
else
  bad "B2a scan_and_store calls resolve_alert"
fi
if grep -qE 'notifications::fire_alert\(' <<< "$STORE"; then
  ok "B2b scan_and_store calls fire_alert"
else
  bad "B2b scan_and_store calls fire_alert — a critical/high finding pages nobody"
fi

# B3: POSITIONAL — resolve before the conditional fire, same shape as A3.
B_RESOLVE_LINE=$(grep -n 'notifications::resolve_alert(' <<< "$STORE" | head -1 | cut -d: -f1)
B_FIRE_LINE=$(grep -n 'if result.critical_count > 0 || result.high_count > 0' <<< "$STORE" | head -1 | cut -d: -f1)
if [ -n "$B_RESOLVE_LINE" ] && [ -n "$B_FIRE_LINE" ] && [ "$B_RESOLVE_LINE" -lt "$B_FIRE_LINE" ]; then
  ok "B3 the resolve (line $B_RESOLVE_LINE) runs unconditionally BEFORE the dirty-branch fire decision (line $B_FIRE_LINE)"
else
  bad "B3 the resolve must precede the fire decision — resolve=${B_RESOLVE_LINE:-none} fire=${B_FIRE_LINE:-none}"
fi

# B4: the alert_type is the new 'image_scan' vocabulary entry, not a reused
# string that would collide with the full-server security scanner's own rows.
FIRE_CALL=$(perl -0777 -ne '
  while (/notifications::fire_alert\s*(\((?:[^()]++|(?1))*\))/gs) {
    my $c = $1; $c =~ s/\s+/ /g; print "$c\n";
  }
' <<< "$STORE")
if grep -qE '"image_scan"' <<< "$FIRE_CALL"; then
  ok "B4 the fired alert uses the dedicated 'image_scan' type"
else
  bad "B4 the fired alert uses the dedicated 'image_scan' type — got: $FIRE_CALL"
fi

# B5: THE PER-IMAGE SCOPING BOUNDARY. state_key must be DERIVED from the image
# reference (via the overflow-safe helper, not the raw string — see §C), not
# an empty string or a server-wide constant — an empty key would collapse
# every image on a server into ONE alert row, so a clean scan of image A would
# resolve a still-dirty image B's alert, and a dirty scan of image B would
# re-fire on top of image A's already-firing row. This is the exact class of
# bug the ownership audit's s301/s302 fixes closed for the DB layer; this
# checks it holds for the new alert-routing call sites too. Flattened, since
# rustfmt may reflow the call across lines.
# Paren-balanced extraction (mirroring alert-controls-pin-e2e.sh's own
# `calls()` helper) — a naive `[^)]*` cannot span the nested `Some(server_id)`
# every fire_alert/resolve_alert call here contains.
STORE_CALLS() {
  perl -0777 -ne '
    while (/\b\Q'"$1"'\E\s*(\((?:[^()]++|(?1))*\))/gs) {
      my $c = $1; $c =~ s/\s+/ /g; print "$c\n";
    }
  ' <<< "$STORE"
}
FIRE_CALLS=$(STORE_CALLS 'notifications::fire_alert')
RESOLVE_CALLS=$(STORE_CALLS 'notifications::resolve_alert')
if grep -qE 'let state_key = image_scan_state_key\(&result\.image\);' <<< "$STORE" \
   && grep -qE '&state_key' <<< "$FIRE_CALLS" \
   && grep -qE '&state_key' <<< "$RESOLVE_CALLS"; then
  ok "B5 the fire/resolve state_key is derived from the image reference via the overflow-safe helper"
else
  bad "B5 the fire/resolve state_key must be derived from the image reference via image_scan_state_key — a raw &result.image would overflow alerts.state_key VARCHAR(100) and silently never fire for a digest-pinned reference"
fi

# B6: admin fan-out exists — scan_and_store is called from a background sweep
# with no HTTP claims, so it cannot target "the user who triggered this" the
# way trigger_scan does; it must resolve who to notify itself.
if grep -qE "SELECT id FROM users WHERE role = 'admin'" <<< "$STORE"; then
  ok "B6 scan_and_store resolves its own admin recipients (no HTTP claims available to the background sweep)"
else
  bad "B6 scan_and_store resolves its own admin recipients — the background sweep path has no caller identity to borrow"
fi

echo
echo "== §C  a WordPress plugin vulnerability scan pages the site's owner, and clears when fixed =="

# C1-control: body extraction. wordpress.rs also has an unrelated `vuln_scan`
# HTTP handler — anchored on the fn signature this shares nothing with.
WP_STORE=$(awk '/^pub async fn scan_and_store\(/{i=1} i{print} i && /^}$/{exit}' "$WP")
NWPSTORE=$(grep -c . <<< "$WP_STORE")
if [ "$NWPSTORE" -ge 30 ]; then
  ok "C1-control wordpress::scan_and_store body extracted — $NWPSTORE lines"
else
  bad "C1-control wordpress::scan_and_store body extracted — only $NWPSTORE lines (the extractor broke)"
fi

# C2: both calls exist.
if grep -qE 'notifications::resolve_alert\(' <<< "$WP_STORE"; then
  ok "C2a wordpress::scan_and_store calls resolve_alert"
else
  bad "C2a wordpress::scan_and_store calls resolve_alert"
fi
if grep -qE 'notifications::fire_alert\(' <<< "$WP_STORE"; then
  ok "C2b wordpress::scan_and_store calls fire_alert"
else
  bad "C2b wordpress::scan_and_store calls fire_alert — a critical/high plugin finding pages nobody"
fi

# C3: POSITIONAL — resolve before the conditional fire, same shape as A3/B3.
C_RESOLVE_LINE=$(grep -n 'notifications::resolve_alert(' <<< "$WP_STORE" | head -1 | cut -d: -f1)
C_FIRE_LINE=$(grep -n 'if critical > 0 || high > 0' <<< "$WP_STORE" | head -1 | cut -d: -f1)
if [ -n "$C_RESOLVE_LINE" ] && [ -n "$C_FIRE_LINE" ] && [ "$C_RESOLVE_LINE" -lt "$C_FIRE_LINE" ]; then
  ok "C3 the resolve (line $C_RESOLVE_LINE) runs unconditionally BEFORE the dirty-branch fire decision (line $C_FIRE_LINE)"
else
  bad "C3 the resolve must precede the fire decision — resolve=${C_RESOLVE_LINE:-none} fire=${C_FIRE_LINE:-none}"
fi

# C4: the alert_type is the dedicated 'wp_vuln_scan' vocabulary entry.
WP_FIRE_CALL=$(perl -0777 -ne '
  while (/notifications::fire_alert\s*(\((?:[^()]++|(?1))*\))/gs) {
    my $c = $1; $c =~ s/\s+/ /g; print "$c\n";
  }
' <<< "$WP_STORE")
if grep -qE '"wp_vuln_scan"' <<< "$WP_FIRE_CALL"; then
  ok "C4 the fired alert uses the dedicated 'wp_vuln_scan' type"
else
  bad "C4 the fired alert uses the dedicated 'wp_vuln_scan' type — got: $WP_FIRE_CALL"
fi

# C5: state_key derived via the overflow-safe helper — same class of bug B5
# guards against, ported to a domain instead of an image reference.
WP_STORE_CALLS() {
  perl -0777 -ne '
    while (/\b\Q'"$1"'\E\s*(\((?:[^()]++|(?1))*\))/gs) {
      my $c = $1; $c =~ s/\s+/ /g; print "$c\n";
    }
  ' <<< "$WP_STORE"
}
WP_FIRE_CALLS=$(WP_STORE_CALLS 'notifications::fire_alert')
WP_RESOLVE_CALLS=$(WP_STORE_CALLS 'notifications::resolve_alert')
if grep -qE 'let state_key = wp_vuln_state_key\(domain\);' <<< "$WP_STORE" \
   && grep -qE '&state_key' <<< "$WP_FIRE_CALLS" \
   && grep -qE '&state_key' <<< "$WP_RESOLVE_CALLS"; then
  ok "C5 the fire/resolve state_key is derived from the domain via the overflow-safe helper"
else
  bad "C5 the fire/resolve state_key must be derived from the domain via wp_vuln_state_key — a raw domain could overflow alerts.state_key VARCHAR(100) and silently never fire"
fi

# C6: PER-SITE-OWNER, not per-admin — the deliberate divergence from image
# scanning's B6. A WordPress site belongs to one user (sites.user_id); firing
# to every admin the way a shared Docker image does would page people who do
# not own the site and, more importantly, is not what this function does —
# this pins the actual design so a future edit that copies B6's admin-fanout
# pattern here (plausible, since the two functions sit side by side) is
# caught rather than silently changing who gets paged.
if grep -qE "SELECT id FROM users WHERE role = 'admin'" <<< "$WP_STORE"; then
  bad "C6 wordpress::scan_and_store must notify the site's OWNER, not fan out to every admin — an admin-fanout query appeared in the body"
elif grep -qE '&state_key' <<< "$WP_FIRE_CALLS" && grep -qE 'user_id,' <<< "$WP_FIRE_CALLS" \
     && grep -qE 'user_id,' <<< "$WP_RESOLVE_CALLS"; then
  ok "C6 the fire/resolve calls target the site's owner (the user_id parameter), not an admin fan-out"
else
  bad "C6 the fire/resolve calls must target the site's owner (the user_id parameter) — call was: $WP_FIRE_CALLS"
fi

# C7-control: the background sweep actually calls the shared function — the
# structural wiring that makes this more than a manual-click-only path, the
# same gap §B's own header calls out for image scanning ("NEITHER the
# manual/deploy-triggered scan NOR the 30-minute background sweep").
if grep -qE 'wordpress::scan_and_store\(' "$WP_SCANNER"; then
  ok "C7 the 30-minute background sweep (wp_vuln_scanner) calls the shared scan_and_store"
else
  bad "C7 the background sweep must call wordpress::scan_and_store — a scan-only sweep with no shared alert path reproduces the original gap on a schedule"
fi

echo
printf 'scan-alert-resolve: \033[32m%d passed\033[0m, \033[31m%d failed\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
