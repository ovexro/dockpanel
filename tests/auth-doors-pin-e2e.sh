#!/usr/bin/env bash
# Regression pins for s277 — every door that mints a session owes the same gates.
#
# A session cookie is a session cookie no matter which handler emits it, but the
# cross-cutting access gates were written per-handler, and by v2.46.0 they had
# drifted into four different opinions about who may sign in:
#
#   D1  THE IP ALLOWLIST GATED ONE DOOR. `allowed_panel_ips` gates login — s276
#       gave it range matching, save-time validation and a control. It lived
#       inline in `login` alone, so an operator who restricted the panel to their
#       office range still had `passkey/auth/*` and `oauth/{provider}/callback`
#       answering from anywhere, both issuing the same cookie. Driven on a real
#       box from an excluded address: the password door returned 403 while
#       passkey/auth/begin returned 200 and a usable WebAuthn challenge.
#   D2  LOCKDOWN SKIPPED THE OAUTH DOOR. Lockdown holds non-admins out until it
#       expires. `login` and `passkey/auth/complete` checked it; `oauth::callback`
#       had zero occurrences of `is_locked_down`, so with SSO configured a
#       lockdown did not apply to the SSO path.
#   D3  THE SECOND-STEP DOOR WAS INVISIBLE. `twofa_verify` is what actually emits
#       the cookie when 2FA is on, holding a 5-minute bearer token from a first
#       request. It re-checked suspension and nothing else, so the temp token
#       could be redeemed from an excluded address or after a lockdown began.
#
# The shape is lesson #104d one layer out: a control's blast radius is what it
# GATES, not what it names — and here the same control had four different radii.
#
# These pins DISCOVER their subjects. Listing the four doors would reproduce the
# defect: the list is complete only until someone adds a fifth. So a door is
# found by what makes it a door — a handler that emits a session cookie — and the
# suite FAILS if it discovers fewer doors than are known to exist, because a
# broken discovery pattern must never read as a clean sweep (lesson #100, and
# #104f, where a line-oriented pattern found 7 of 8 subjects and three arms below
# it passed vacuously).
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

BE=panel/backend/src
ROUTES=$BE/routes

[ -d "$ROUTES" ] || { echo "missing dir: $ROUTES"; exit 1; }

# NOTE ON grep: counts use `grep -c`, never `grep -q` in a pipeline. Under
# `set -o pipefail` a `grep -q` exits at the first match, the producer dies of
# SIGPIPE 141, and the pipeline reports FAILURE for a SUCCESSFUL match (#103d).
#
# NOTE ON LINE ORIENTATION: bodies are sliced with perl -0777, not grep. A gate
# call wrapped across lines is invisible to a line-at-a-time pattern, which is
# exactly how #104f's arm found 7 of 8 knobs.

# ── Discovery ───────────────────────────────────────────────────────────────
#
# A door is a `pub async fn` whose body emits an auth session cookie. We look for
# the two ways the tree does that: calling the shared issuer, or formatting the
# `token={token}` cookie inline. Logout is excluded by construction — it emits
# `token=;` to CLEAR the cookie, and clearing a session is not minting one.

DOORS=$(perl -0777 -ne '
  while (/pub async fn\s+(\w+)\s*\(/g) {
    my $name = $1;
    my $start = pos($_);
    my $rest = substr($_, $start);
    # body = up to the next top-level `pub async fn` / `pub fn` / end of file
    my ($body) = $rest =~ /^(.*?)(?=\npub (?:async )?fn |\z)/s;
    next unless defined $body;
    next unless $body =~ /issue_session(?:_pub)?\s*\(/
             || $body =~ /"token=\{token\}/;
    print "$ARGV:$name\n";
  }
' $(ls "$ROUTES"/*.rs) 2>/dev/null | sort -u)

DOORS_N=$(printf '%s\n' "$DOORS" | grep -c . || true)

echo "── discovery ──"
printf '%s\n' "$DOORS" | sed 's/^/  door: /'

# §0 — the discovery itself. Four doors are known to exist (login, twofa_verify,
# passkeys::auth_complete, oauth::callback). Fewer means the pattern broke and
# every arm below would pass by inspecting nothing.
if [ "$DOORS_N" -ge 4 ]; then
  ok "discovered $DOORS_N session-minting doors (>= 4 known to exist)"
else
  bad "only $DOORS_N doors discovered — the mint pattern moved; every arm below is now vacuous"
fi

# ── §1 every door enforces the panel IP allowlist ───────────────────────────
echo "── §1 allowed_panel_ips covers every door ──"
for d in $DOORS; do
  file=${d%%:*}; fn=${d##*:}
  body=$(perl -0777 -ne "
    if (/pub async fn\s+\Q$fn\E\s*\(/g) {
      my \$rest = substr(\$_, pos(\$_));
      my (\$b) = \$rest =~ /^(.*?)(?=\npub (?:async )?fn |\z)/s;
      print \$b;
    }" "$file")
  n=$(printf '%s' "$body" | grep -c "enforce_panel_ip_allowlist" || true)
  if [ "$n" -ge 1 ]; then
    ok "$(basename "$file")::$fn enforces the IP allowlist"
  else
    bad "$(basename "$file")::$fn mints a session WITHOUT the IP allowlist — an excluded address can sign in here"
  fi
done

# ── §2 every door enforces lockdown (with the admin escape hatch) ────────────
echo "── §2 lockdown covers every door ──"
for d in $DOORS; do
  file=${d%%:*}; fn=${d##*:}
  body=$(perl -0777 -ne "
    if (/pub async fn\s+\Q$fn\E\s*\(/g) {
      my \$rest = substr(\$_, pos(\$_));
      my (\$b) = \$rest =~ /^(.*?)(?=\npub (?:async )?fn |\z)/s;
      print \$b;
    }" "$file")
  n=$(printf '%s' "$body" | grep -c "is_locked_down" || true)
  if [ "$n" -ge 1 ]; then
    ok "$(basename "$file")::$fn enforces lockdown"
  else
    bad "$(basename "$file")::$fn mints a session WITHOUT the lockdown check — lockdown does not hold this door"
  fi
done

# ── §3 the allowlist lives in ONE place ─────────────────────────────────────
# The defect was a gate written inline in one handler. If a second inline copy of
# the settings lookup reappears, the two will drift exactly as the settings
# whitelist did (#104g), so the query belongs to the helper alone.
echo "── §3 one implementation, not a copy per door ──"
INLINE=$(grep -rl "allowed_panel_ips" --include=*.rs "$ROUTES" | sort -u)
INLINE_N=$(printf '%s\n' "$INLINE" | grep -c . || true)
# auth.rs holds the helper; settings.rs validates the value on save. Any third
# route file touching the key is an inline re-implementation.
UNEXPECTED=$(printf '%s\n' "$INLINE" | grep -v -e 'routes/auth\.rs$' -e 'routes/settings\.rs$' || true)
if [ -z "$UNEXPECTED" ]; then
  ok "allowed_panel_ips is referenced only by the helper (auth.rs) and its validator (settings.rs)"
else
  bad "inline allowlist re-implementation in: $(printf '%s ' $UNEXPECTED) — it will drift from the helper"
fi

# The helper must fail closed. `panel_ip_allowed` returns false for an
# unparseable client address; a well-meaning `unwrap_or(true)` here would admit
# every request that arrives without X-Real-IP.
HELPER_OK=$(perl -0777 -ne 'print "1" if /fn panel_ip_allowed.*?let Ok\(client\).*?else \{.*?return false;/s' "$BE/helpers.rs")
if [ -n "$HELPER_OK" ]; then
  ok "panel_ip_allowed fails CLOSED when the client address is unusable"
else
  bad "panel_ip_allowed no longer visibly fails closed on an unparseable client address"
fi

# ── §4 ingestion preserves when an event HAPPENED ───────────────────────────
# Auto-lockdown counts "N events within M minutes" over created_at. Stamping an
# ingested backlog with NOW() collapses however long it took to accumulate into
# one instant. Before v2.46.0 ingestion sat under `auto_heal_enabled` (seeded
# false), so a stock box queued events on disk that nothing drained — and the
# first tick after upgrading replayed the whole file as simultaneous and locked
# every non-admin out. Driven on a real box: 16 events spread over ~3 minutes of
# real time landed within 105ms of each other, and the tenant got a 503.
echo "── §4 suspicious-event ingestion honours the event's own timestamp ──"
HEALER=$BE/services/auto_healer.rs
n=$(perl -0777 -ne 'print "1" if /record_suspicious_event_at\s*\(/s' "$HEALER" | grep -c 1 || true)
if [ "$n" -ge 1 ]; then
  ok "the jsonl ingest records events at their occurrence time, not at ingest time"
else
  bad "the ingest no longer passes an occurrence time — a queued backlog will trip auto-lockdown on the first tick"
fi
n=$(perl -0777 -ne 'print "1" if /event\["timestamp"\]/s' "$HEALER" | grep -c 1 || true)
if [ "$n" -ge 1 ]; then
  ok "the agent's per-event timestamp is read (the agent has always written it)"
else
  bad "the ingest ignores the agent's timestamp field — the writer/reader pair is severed again"
fi

# ── §5 the recording toggle cannot claim more than the fleet delivers ───────
# The toggle is one settings row enforced by each server's agent: an agent older
# than the gate release ignores the signed claim and keeps recording. The panel
# already knows every member's version (servers.agent_version, written on each
# check-in), so reporting a flat "disabled" is a claim it can itself falsify —
# lesson #104a, one layer out.
echo "── §5 the recording toggle discloses fleet members it cannot control ──"
n=$(grep -c "recording_coverage" "$ROUTES/settings.rs" || true)
if [ "$n" -ge 1 ]; then
  ok "backend exposes recording coverage across the fleet"
else
  bad "no recording-coverage endpoint — the toggle makes a fleet-wide claim nothing checks"
fi
n=$(grep -rc "recording-coverage" panel/frontend/src/pages/Settings.tsx || true)
if [ "$n" -ge 1 ]; then
  ok "the Settings toggle consumes it, so lagging members are named in the UI"
else
  bad "the UI never reads recording coverage — operators are told 'disabled' while members still record"
fi

echo
if [ "$FAIL" -eq 0 ]; then
  printf '\033[0;32mauth-doors pins: %d passed\033[0m\n' "$PASS"
  exit 0
else
  printf '\033[0;31mauth-doors pins: %d passed, %d FAILED\033[0m\n' "$PASS" "$FAIL"
  exit 1
fi
