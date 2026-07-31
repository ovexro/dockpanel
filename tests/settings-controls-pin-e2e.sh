#!/usr/bin/env bash
# Regression pins for s276 — settings that lie about what they control.
#
# The panel keeps operator configuration in one `settings` table, and three
# independent places have an opinion about each key: the backend READS it, the
# API decides whether it may be WRITTEN, and the frontend may render a CONTROL
# for it. Nothing forced those three to agree, and by v2.45.1 they had drifted
# in both directions at once:
#
#   D1  INERT CONTROLS. `security_session_recording` and `security_canary_enabled`
#       had toggles that saved, reported success, and were read by nothing. The
#       features ran unconditionally, so switching session recording OFF changed
#       nothing while the panel said "Session recording disabled" and the agent
#       kept writing every keystroke to disk. `timezone`, `email_footer` and
#       `events_webhook_url` were the same shape with no feature behind them at
#       all — the timezone select claimed to "affect displayed timestamps
#       throughout the panel" and nothing anywhere read the key.
#   D2  UNSETTABLE GATES. `allowed_panel_ips` guarded login by IP and the docs
#       told operators to set it in Settings — where there was no control, and
#       the API answered 400 Unknown setting. `server_terminal_disabled`,
#       `security_lockdown_window_minutes` and `security_site_rate_limit` were
#       the same: read for behaviour, writable by nobody.
#   D3  A SEED THAT DISABLED ITS OWN FEATURE. The migration seeds
#       `security_site_rate_limit` = '3', but the reader was `get_setting_bool`,
#       so '3' != "true" read as FALSE and the ceiling became 999 per hour. The
#       absent row meant 3; the seeded row meant off. Every install that ran that
#       migration had the limit disabled while the row said "3".
#   D4  A CONTROL THAT WOULD RENDER THE WRONG DEFAULT. Keys default in opposite
#       directions, so a toggle copied from its neighbour draws OFF on a fresh
#       install while the server is answering yes — a control showing "off" ends
#       the investigation. See lesson #103f.
#
# These pins DISCOVER their subjects. Naming the keys would reproduce the defect
# that created them: a checklist grows by one line each time a miss is found, and
# the next key added to the codebase is not on it. So the allowlist is parsed out
# of the source, the configurable knobs are found by the helper that reads them,
# and every discovery arm FAILS when it finds fewer subjects than are known to
# exist — a broken pattern must not read as a clean sweep (lesson #100).
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

BE=panel/backend/src
FE=panel/frontend/src
SETTINGS_RS=$BE/routes/settings.rs

for f in "$SETTINGS_RS" "$FE/pages/Settings.tsx"; do
  [ -f "$f" ] || { echo "missing file: $f"; exit 1; }
done

# NOTE ON grep: every count below uses `grep -c`, never `grep -q` in a pipeline.
# Under `set -o pipefail`, `grep -q` exits at the first match, the upstream
# producer dies of SIGPIPE 141, and the pipeline reports FAILURE for a
# SUCCESSFUL match — selectively, depending on where in the file the match sits.
# That silently cost this suite's sibling three of five subjects (lesson #103d).

# ── Discovery ───────────────────────────────────────────────────────────────

# The writable whitelist, read out of the source rather than restated here.
ALLOWED=$(sed -n '/pub const ALLOWED_KEYS/,/^\];/p' "$SETTINGS_RS" \
          | grep -oE '"[a-z0-9_]+"' | tr -d '"' | sort -u)
ALLOWED_N=$(printf '%s\n' "$ALLOWED" | grep -c . || true)

# Every settings key the backend reads, multi-line aware: the SQL is written with
# backslash continuations, so a line-at-a-time grep silently misses whole IN(...)
# lists. It missed image_scan_* and prometheus_enabled when this sweep began.
BE_FILES=$(grep -rl "FROM settings" --include=*.rs "$BE" || true)
READ_KEYS=$(perl -0777 -pe 's/\\\s*\n\s*//g' $BE_FILES 2>/dev/null \
            | grep -oE "key (=|IN|= ANY) *\(?[^)]*" \
            | grep -oE "'[a-z0-9_]+'" | tr -d "'" | sort -u)
READ_N=$(printf '%s\n' "$READ_KEYS" | grep -c . || true)

# Keys read through the helpers that exist precisely to express "operator knob
# with a server default". Discovered by the call, so a new knob joins by existing.
#
# Read with perl -0777, NOT grep: a call formatted across several lines is
# invisible to a line-oriented pattern. One of these calls is wrapped, and a
# line-based version of this arm found 7 of 8 while every check below it still
# printed green — the same partial-discovery failure the suite exists to catch.
# Emits "<key> <default>" so §3 can ask each knob which way it defaults.
KNOB_PAIRS=$(perl -0777 -ne 'while (/get_setting_(?:bool|i64)\s*\(\s*[^,]+,\s*"([a-z0-9_]+)"\s*,\s*([A-Za-z0-9_]+)/gs) { print "$1 $2\n" }' \
             $(grep -rl "get_setting_" --include=*.rs "$BE") 2>/dev/null | sort -u)
KNOBS=$(printf '%s\n' "$KNOB_PAIRS" | awk 'NF{print $1}' | sort -u)
KNOBS_N=$(printf '%s\n' "$KNOBS" | grep -c . || true)

# The frontend tree with comments removed, which is what §9 searches.
#
# A pin that greps raw source matches PROSE. Three of the cards this suite now
# guards describe the keys they set in a comment above themselves, and the 503
# from routes/billing.rs quotes a key name verbatim — so a key deleted from the
# JSX but still named in the paragraph explaining it would keep reading as
# "controlled" forever. That is the failure this suite exists to catch, arriving
# through the suite's own front door.
#
# Block comments go because that is where JSX prose lives ({/* … */}); `//` is
# stripped only at the start of a line, never mid-line, or the `//` of every
# https:// URL would take the rest of its line with it — including, in this
# tree, the line holding a provider's console link.
FE_SRC=$(mktemp)
trap 'rm -f "$FE_SRC"' EXIT
find "$FE" -name '*.tsx' -exec cat {} + \
  | perl -0777 -pe 's{\{/\*.*?\*/\}}{}gs; s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms' \
  | grep -vE '^\s*//' > "$FE_SRC"

# Keys constructed at the call site rather than spelled out, e.g.
# `oauth_${p.id}_client_id` or `notif_template_${c.id}`. Recorded as
# "<prefix> <suffix>" so a key can be credited when it matches both ends; the
# families this covers are exactly the ones a literal grep cannot see.
FE_TEMPLATES=$(grep -ohE '`[a-z0-9_]+\$\{[^}]*\}[a-z0-9_]*`' "$FE_SRC" \
               | sed -E 's/^`([a-z0-9_]+)\$\{[^}]*\}([a-z0-9_]*)`$/\1 \2/' | sort -u)

# Keys a purpose-built backend route sets on the operator's behalf, so the panel
# offers a control that never PUTs to /api/settings. `reverse_proxy` is the live
# example: the nginx/Traefik selector calls /traefik/install, and routes/system.rs
# writes the key. Discovered rather than exempted — an exemption list is the
# checklist this suite's header refuses to grow.
BE_WRITTEN=$(grep -rhoE "INSERT INTO settings \(key[^)]*\) VALUES \('[a-z0-9_]+'" --include=*.rs "$BE" \
             | grep -oE "'[a-z0-9_]+'" | tr -d "'" | sort -u)
BE_WRITTEN_N=$(printf '%s\n' "$BE_WRITTEN" | grep -c . || true)

echo "── discovery ──"
echo "  writable keys: $ALLOWED_N   backend-read keys: $READ_N   get_setting_* knobs: $KNOBS_N"

# §0 — the discovery itself. Fewer subjects than are known to exist means a
# pattern broke, and every arm below would then pass by looking at nothing.
if [ "$ALLOWED_N" -ge 40 ]; then
  ok "parsed $ALLOWED_N writable keys out of ALLOWED_KEYS (>= 40 known to exist)"
else
  bad "only $ALLOWED_N writable keys parsed — ALLOWED_KEYS moved or changed shape; every arm below is now vacuous"
fi

if [ "$READ_N" -ge 30 ]; then
  ok "discovered $READ_N settings keys read by the backend (>= 30 known to exist)"
else
  bad "only $READ_N backend-read keys discovered — the multi-line SQL scan broke; arms below are vacuous"
fi

if [ "$KNOBS_N" -ge 8 ]; then
  ok "discovered $KNOBS_N configurable knobs via get_setting_bool/_i64 (>= 8 known to exist)"
else
  bad "only $KNOBS_N get_setting_* knobs discovered — the call pattern changed; §2 and §3 are now vacuous"
fi

# ── §1 — no writable key that nothing reads (the D1 class) ──────────────────
# A key an operator can set must be consumed somewhere. The ALLOWED_KEYS block
# is stripped first, or every key would trivially find itself.
echo "── §1 every writable key is read by something ──"
REST=$(mktemp)
sed '/pub const ALLOWED_KEYS/,/^\];/d' "$SETTINGS_RS" > "$REST"
INERT=""
for k in $ALLOWED; do
  n=$(grep -rlF "$k" --include=*.rs "$BE" | grep -v "routes/settings.rs" | grep -c . || true)
  m=$(grep -cF "$k" "$REST" || true)
  # Families built at the call site (`format!("notif_template_{channel}")`) never
  # appear whole; credit the key when its prefix is constructed somewhere.
  p=$(grep -rhoE 'format!\("[a-z0-9_]+\{' --include=*.rs "$BE" \
      | grep -oE '"[a-z0-9_]+' | tr -d '"' \
      | while read -r pre; do case "$k" in "$pre"*) echo hit ;; esac; done | grep -c . || true)
  if [ "$n" -eq 0 ] && [ "$m" -eq 0 ] && [ "$p" -eq 0 ]; then
    INERT="$INERT $k"
  fi
done
rm -f "$REST"
if [ -z "$INERT" ]; then
  ok "every writable key is consumed by backend code"
else
  bad "writable but read by NOTHING —$INERT — the control saves and reports success while the feature ignores it (D1)"
fi

# ── §2 — every knob is settable (the D2 class) ──────────────────────────────
# A key read through get_setting_bool/_i64 is by construction operator
# configuration with a default. If it is not writable, the default is all there
# will ever be, and the panel offers no way to change it.
echo "── §2 every configurable knob is writable ──"
STUCK=""
for k in $KNOBS; do
  # -x: ALLOWED is newline-separated, and a substring test would also let
  # `security_lockdown_window_minutes` be satisfied by an unrelated longer key.
  in_list=$(printf '%s\n' "$ALLOWED" | grep -cxF "$k" || true)
  [ "$in_list" -eq 0 ] && STUCK="$STUCK $k"
done
if [ -z "$STUCK" ]; then
  ok "every get_setting_* knob appears in ALLOWED_KEYS"
else
  bad "read as configuration but writable by nobody —$STUCK — permanently stuck at its default (D2)"
fi

# ── §3 — controls render the SERVER's default, not a neighbour's (D4) ───────
# `get_setting_bool(db, "k", true)` means an absent row reads TRUE. Its control
# must therefore test `!== "false"`; `=== "true"` would draw it OFF on every
# install that never set the row, while the server keeps answering yes. That is
# worse than no control, because a control showing "off" ends the investigation.
echo "── §3 default-open toggles are not compared with === \"true\" ──"
OPEN_KNOBS=$(printf '%s\n' "$KNOB_PAIRS" | awk '$2=="true"{print $1}' | sort -u)
OPEN_N=$(printf '%s\n' "$OPEN_KNOBS" | grep -c . || true)
if [ "$OPEN_N" -ge 4 ]; then
  ok "discovered $OPEN_N default-open boolean knobs (>= 4 known to exist)"
else
  bad "only $OPEN_N default-open knobs discovered — the arm below is vacuous"
fi
LIARS=""
for k in $OPEN_KNOBS; do
  # Only judge keys the frontend actually renders; a knob with no control is §1/§2's job.
  present=$(grep -rlF "settings.$k" --include=*.tsx "$FE" | grep -c . || true)
  [ "$present" -eq 0 ] && continue
  wrong=$(grep -rhoE "settings\.$k === \"true\"" --include=*.tsx "$FE" | grep -c . || true)
  right=$(grep -rhoE "settings\.$k !== \"false\"" --include=*.tsx "$FE" | grep -c . || true)
  if [ "$wrong" -gt 0 ] || [ "$right" -eq 0 ]; then
    LIARS="$LIARS $k"
  fi
done
if [ -z "$LIARS" ]; then
  ok "every rendered default-open toggle tests !== \"false\", matching its server default"
else
  bad "control disagrees with the server's own default —$LIARS — renders OFF while the server answers yes (D4)"
fi

# ── §4 — the seed and the reader must agree on the TYPE (D3) ────────────────
# A key seeded with a number must not be read as a boolean. '3' != "true", so the
# seed silently disabled the feature it configures.
echo "── §4 numeric seeds are not read as booleans ──"
SEEDS=$(grep -rhoE "\('[a-z0-9_]+', *'[0-9]+'\)" panel/backend/migrations/*.sql \
        | grep -oE "'[a-z0-9_]+'" | head -n -0 | tr -d "'" | sort -u | grep -vE '^[0-9]+$' || true)
SEEDS_N=$(printf '%s\n' "$SEEDS" | grep -c . || true)
if [ "$SEEDS_N" -ge 3 ]; then
  ok "discovered $SEEDS_N numerically-seeded settings keys (>= 3 known to exist)"
else
  bad "only $SEEDS_N numeric seeds discovered — the migration scan broke; the arm below is vacuous"
fi
MISTYPED=""
for k in $SEEDS; do
  b=$(grep -rhoE "get_setting_bool\([^)]*\"$k\"" --include=*.rs "$BE" | grep -c . || true)
  [ "$b" -gt 0 ] && MISTYPED="$MISTYPED $k"
done
if [ -z "$MISTYPED" ]; then
  ok "no numerically-seeded key is read with get_setting_bool"
else
  bad "seeded with a NUMBER but read as a BOOLEAN —$MISTYPED — the seed disables the feature it configures (D3)"
fi

# ── §5 — the whitelist exists once (it had already drifted) ────────────────
# `update` and `import_config` each spelled the list out, and import's copy was
# missing the registration gates and every security_* toggle — so exporting a
# config and importing it silently dropped the security posture, under a comment
# claiming it used "the same whitelist as update()".
echo "── §5 one whitelist, not two ──"
LISTS=$(grep -cE 'let allowed_keys = \[' "$SETTINGS_RS" || true)
USES=$(grep -cE 'ALLOWED_KEYS' "$SETTINGS_RS" || true)
if [ "$LISTS" -eq 0 ] && [ "$USES" -ge 3 ]; then
  ok "both writers reference the single ALLOWED_KEYS const ($USES references, 0 inline copies)"
else
  bad "found $LISTS inline key list(s) in settings.rs — a second copy drifts from the first, as it already did once"
fi

# ── §6 — the IP allowlist matches by range, and validates on write ──────────
# The docs promised CIDR ranges while the code compared strings, so a CIDR entry
# matched nothing and locked the operator out instead of restricting access.
echo "── §6 the panel IP allowlist ──"
if [ "$(grep -cF 'panel_ip_allowed' "$BE/routes/auth.rs" || true)" -gt 0 ]; then
  ok "login checks the allowlist through panel_ip_allowed, not string equality"
else
  bad "routes/auth.rs no longer calls panel_ip_allowed — a CIDR entry silently matches nothing"
fi
if [ "$(grep -cF 'valid_panel_ip_entry' "$SETTINGS_RS" || true)" -gt 0 ]; then
  ok "a malformed entry is rejected on save, so a typo cannot lock every admin out"
else
  bad "allowed_panel_ips is stored unvalidated — a typo locks everyone out and only a DB edit recovers it"
fi

# ── §7 — session recording is decided by the panel, in the SIGNED ticket ────
# The browser holds the ticket and dials the agent directly. Carried as a query
# param, a user could switch off the recording of their own session.
echo "── §7 session recording is a signed decision ──"
AGENT_TERM=panel/agent/src/routes/terminal.rs
if [ "$(grep -cE 'record: Option<bool>' "$AGENT_TERM" || true)" -gt 0 ] \
   && [ "$(grep -cE 'record: *bool' "$BE/routes/terminal.rs" || true)" -gt 0 ]; then
  ok "the record flag rides inside the JWT ticket on both sides"
else
  bad "the record flag is not a ticket claim — if it moved to a query param, users can unrecord themselves"
fi
# Anchor on the OPERATION, not a mention: every one of these files also *names*
# recording in a comment or a path, so a presence check passes on a file that
# stopped doing the thing (lesson #103e).
if [ "$(grep -cE 'if record \{' "$AGENT_TERM" || true)" -gt 0 ]; then
  ok "the agent opens the .cast file only when the ticket allows it"
else
  bad "the agent's recording open is no longer gated on the flag — the toggle is inert again"
fi
if [ "$(perl -0777 -ne 'print scalar(() = /get_setting_bool\s*\(\s*&state\.db,\s*"security_session_recording"/gs)' "$BE/routes/terminal.rs")" -gt 0 ]; then
  ok "the panel reads security_session_recording when minting the ticket"
else
  bad "the panel no longer reads security_session_recording — the toggle stops reaching the agent"
fi

# ── §8 — security monitoring does not ride on the auto-heal switch ─────────
# `auto_heal_enabled` names auto-healing. It also gated suspicious-event
# ingestion, lockdown expiry and canary monitoring, so turning off auto-healing
# silently turned off the security monitoring an operator never asked to stop.
echo "── §8 security tasks are independent of auto_heal_enabled ──"
HEALER=$BE/services/auto_healer.rs
if [ "$(grep -cE 'if is_enabled\(&pool\)\.await \{' "$HEALER" || true)" -gt 0 ] \
   && [ "$(grep -cE '^\s*if !enabled \{' "$HEALER" || true)" -eq 0 ]; then
  ok "auto-healing is scoped to its own block instead of skipping the rest of the tick"
else
  bad "the healer still 'continue's on !enabled — everything after it, including the security tasks, is gated by that switch"
fi
if [ "$(grep -cE 'get_setting_bool\(&pool, "security_canary_enabled"' "$HEALER" || true)" -gt 0 ]; then
  ok "canary monitoring consults its own toggle"
else
  bad "security_check_canary_files is unconditional again — its toggle is inert"
fi

# ── §9 — every writable key has an operator control (the D2 class, UI half) ──
# ALLOWED_KEYS' own comment has promised since s276 that "every entry should also
# have a control in the panel — tests/settings-controls-pin-e2e.sh fails when one
# does not". It did not. This suite computed the frontend's key list into a
# variable and then never read it, so the half of D2 that says "read for
# behaviour, writable by nobody" was pinned while the half that says "writable,
# but by an API call nobody can make from the panel" was not — and fourteen keys
# had drifted through the gap: the six oauth_*_client_id/_client_secret behind a
# real login door, the four notification templates, the three Stripe price IDs,
# and hide_branding, whose value four components honoured and no screen set.
#
# A control counts when the frontend SENDS the key, not merely when it mentions
# it: as an object-literal key (`{ panel_name: panelName }`), as an index
# assignment (`body.pdns_api_key = …`), or built from a prefix and suffix around
# a template hole (`oauth_${p.id}_client_id`) — plus the backend-route clause
# above. A read is deliberately not enough, and hide_branding is why: the panel
# fetched it, four components changed their rendering on it, and no screen could
# set it. An arm that accepted `data.hide_branding` as proof of a control would
# have called that key controlled for as long as it stayed broken.
#
# Being sent is necessary, not sufficient — §1/§3 judge whether the control then
# tells the truth. This arm only answers "can an operator reach it at all".
echo "── §9 every writable key has a control in the panel ──"
if [ "$BE_WRITTEN_N" -ge 8 ]; then
  ok "discovered $BE_WRITTEN_N settings keys written by purpose-built backend routes (>= 8 known to exist)"
else
  bad "only $BE_WRITTEN_N backend-route-written keys discovered — the INSERT scan broke; keys set outside settings.rs will now read as uncontrolled"
fi
UNCONTROLLED=""
CONTROLLED_N=0
for k in $ALLOWED; do
  # `[^:]` after the colon keeps TypeScript's `foo::bar` and `a ? b : c` out; the
  # leading class keeps `data.smtp_host` (a read) from matching as a write.
  w=$(grep -cE "(^|[^A-Za-z0-9_.\"])\"?$k\"? *:[^:]" "$FE_SRC" || true)
  a=$(grep -cE "\.$k *=[^=]" "$FE_SRC" || true)
  hit=$((w + a))
  if [ "$hit" -eq 0 ] && [ -n "$FE_TEMPLATES" ]; then
    while read -r pre suf; do
      [ -z "$pre" ] && continue
      if [[ "$k" == "$pre"*"$suf" ]]; then hit=1; break; fi
    done <<< "$FE_TEMPLATES"
  fi
  # -c, never -q: under pipefail a `grep -q` match SIGPIPEs printf and the
  # pipeline reports failure for a successful match. Same reason as §2.
  be=$(printf '%s\n' "$BE_WRITTEN" | grep -cxF "$k" || true)
  [ "$hit" -eq 0 ] && [ "$be" -gt 0 ] && hit=1
  if [ "$hit" -eq 0 ]; then
    UNCONTROLLED="$UNCONTROLLED $k"
  else
    CONTROLLED_N=$((CONTROLLED_N+1))
  fi
done
if [ -z "$UNCONTROLLED" ]; then
  ok "all $CONTROLLED_N writable keys are settable from the panel — none is API-only"
else
  bad "writable but with NO control in the panel —$UNCONTROLLED — configurable only by hand-crafting a PUT, which is not a feature an operator has (D2)"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
