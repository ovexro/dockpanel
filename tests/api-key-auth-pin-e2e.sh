#!/usr/bin/env bash
# api-key-auth-pin-e2e.sh — s467
#
# Wires the panel's previously-unwired `dp_`-prefixed API key scaffold
# (routes/api_keys.rs) into `AuthUser` (auth.rs), so it authenticates like a
# session JWT on every AuthUser-gated handler — the missing half that let the
# CLI (panel/cli/src/backend_client.rs) restore a site's DATABASE as well as
# its files, which the agent alone structurally cannot do (no DB
# credentials). Full design: project_dockpanel_tech_debt_p198. Full account
# of what shipped, including three fixes a 3-lens adversarial skeptic
# workflow caught before shipping: project_dockpanel_tech_debt_p206.
#
# What the skeptic caught, each pinned below:
#   §A5 — a bad/expired key must NOT use the `session_invalid()` marker (that
#     means "your BROWSER session died, go log in"; a `dp_` key is a
#     session-less bearer credential, same class as an agent token).
#   §A6/A7 — "Revoke all sessions" and the Panic Button both promise a total
#     logout; a `dp_` key must die too if it was minted BEFORE that
#     revocation fired (comparing its own `created_at`, since it has no
#     `iat` of its own to re-derive).
#   §B — the hot-path lookup column (`key_hash`) needs an index, or every
#     API-key request forces a sequential scan of the whole table.
#   §C — the CLI's SSE-stream reader must decide pass/fail from the
#     TERMINAL "complete" step's own status only, matching the panel UI's
#     own rule — not from whether ANY earlier step carried status="error"
#     (the restore handler emits a non-fatal advisory that way on purpose).
#   §C (byte safety) — decoding UTF-8 per raw chunk, before a full frame has
#     arrived, can corrupt a multi-byte character split across a chunk
#     boundary.
#   §D — the CLI's domain→site-id lookup (GET /api/sites, owner-only) is
#     narrower than the restore endpoints it feeds (SITE_CALLER_PREDICATE:
#     owner OR admin-of-host) — needs a GET /api/admin/sites fallback.
#
# Live-fire proof, not just these source pins: a throwaway Vultr VPS ran the
# real published v2.220.0 install, swapped in these locally-built api+cli
# binaries, and drove every one of the above through the CLI's own restore
# command — see project_dockpanel_tech_debt_p206 for the full transcript
# (marker-row DB restore, revoked-key-then-remint, the files-only-metadata/
# real-DB-dump-in-archive scenario that reproduced the false "Restore
# failed" bug pre-fix, and an admin's key restoring a site owned by a
# DIFFERENT user via the admin-sites fallback).
#
# No running panel needed.
#   run: bash tests/api-key-auth-pin-e2e.sh
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

echo
echo "=================================================="
echo "  dp_ API-key auth wiring — source pins (s467)"
echo "=================================================="
echo

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }

# Strip line (`//`, `///`) and block (`/* ... */`, one line only) comments so
# a doc comment mentioning a pinned literal can't satisfy an assertion the
# real code doesn't.
code() {
  [ -f "$1" ] || return 0
  perl -0777 -pe '
    s{^[ \t]*/\*.*?\*/[ \t]*$}{}gms;
    s{^\s*//.*$}{}gm;
    s{^\s*///.*$}{}gm;
  ' "$1"
}

has()   { grep -qE -- "$2" <<< "$1"; }
lacks() { ! grep -qE -- "$2" <<< "$1"; }

# Extract one function's body by brace depth, so an arm scoped to a function
# can't be satisfied by an unrelated occurrence elsewhere in the same file.
fnbody() {
  awk -v fn="$2" '
    index($0, "fn " fn "(") && !started { started=1 }
    started {
      n=gsub(/\{/,"{"); m=gsub(/\}/,"}"); depth += n - m; print
      if (opened || n>0) opened=1
      if (opened && depth<=0) exit
    }
  ' <<< "$1"
}

AUTH=panel/backend/src/auth.rs
AUTH_C=$(code "$AUTH")
MIGRATIONS_GLOB=$(ls panel/backend/migrations/*api_keys_hash*.sql 2>/dev/null | head -1)
MIGRATION_C=$([ -n "$MIGRATIONS_GLOB" ] && code "$MIGRATIONS_GLOB" || echo "")
BACKEND_CLIENT=panel/cli/src/backend_client.rs
BACKEND_CLIENT_C=$(code "$BACKEND_CLIENT")
BACKUP=panel/cli/src/commands/backup.rs
BACKUP_C=$(code "$BACKUP")

# ── §A auth.rs: the dp_ branch and authenticate_api_key() ──────────────────
echo "── §A auth.rs: dp_-prefixed bearer tokens authenticate as an API key, not a JWT ──"

FRP_BODY=$(fnbody "$AUTH_C" "from_request_parts")
if has "$FRP_BODY" 'bearer_token\.as_deref\(\)\.filter\(\|t\| t\.starts_with\("dp_"\)\)'; then
  ok "A1 AuthUser::from_request_parts detects a dp_-prefixed bearer token"
else
  bad "A1 the dp_-prefix detection branch is missing from AuthUser::from_request_parts"
fi

DP_CHECK_LINE=$(grep -nE 'starts_with\("dp_"\)' "$AUTH" | head -1 | cut -d: -f1)
COOKIE_FALLBACK_LINE=$(grep -nE '^\s*let token = bearer_token\.clone\(\)\.or_else' "$AUTH" | head -1 | cut -d: -f1)
if [ -n "$DP_CHECK_LINE" ] && [ -n "$COOKIE_FALLBACK_LINE" ] && [ "$DP_CHECK_LINE" -lt "$COOKIE_FALLBACK_LINE" ]; then
  ok "A2 the dp_ check runs BEFORE the JWT/cookie-fallback logic (an early return, not a fallback)"
else
  bad "A2 the dp_ check is not positioned before the JWT/cookie path — it would never be reached, or would run after a wasted decode attempt"
fi

if has "$AUTH_C" 'async fn authenticate_api_key\(state: &AppState, key: &str\) -> Result<Claims, ApiError>'; then
  ok "A3 authenticate_api_key() exists with the expected signature"
else
  bad "A3 authenticate_api_key() is missing or its signature changed"
fi

AAK_BODY=$(fnbody "$AUTH_C" "authenticate_api_key")

if lacks "$AAK_BODY" 'session_invalid\('; then
  ok "A4 authenticate_api_key() never uses the session_invalid() marker (a key is not a browser session)"
else
  bad "A4 authenticate_api_key() calls session_invalid() — a bad/revoked key would wrongly redirect a browser to /login"
fi

# Control: session_invalid() itself still exists and is still used by the JWT
# path — proves A4 passes because the call was REMOVED from this function
# specifically, not because the whole helper was deleted project-wide.
if has "$AUTH_C" 'fn session_invalid\(msg: &str\) -> ApiError' && has "$AUTH_C" 'session_invalid\("Authentication required"\)'; then
  ok "A4-control session_invalid() still exists and still gates the JWT/cookie path"
else
  bad "A4-control session_invalid() itself was removed or the JWT path no longer uses it"
fi

if has "$AAK_BODY" 'SELECT id, user_id, created_at FROM api_keys WHERE key_hash = \$1'; then
  ok "A5 the key lookup selects id, user_id AND created_at (created_at feeds the revocation check below)"
else
  bad "A5 the key-lookup query is missing created_at — the revocation check below cannot compare against a mint time"
fi

if has "$AAK_BODY" 'state\.sessions_revoked_at\.read\(\)\.await' \
  && has "$AAK_BODY" 'created_at\.timestamp\(\) < ts'; then
  ok "A6 a key minted BEFORE the last 'revoke all sessions'/Panic Button fire is refused"
else
  bad "A6 the sessions_revoked_at check is missing — a stolen/leaked key would survive an incident-response logout that explicitly promises to kill it"
fi

if has "$AAK_BODY" 'role == "suspended"'; then
  ok "A7 a suspended account's key stops authenticating (parity with the JWT path)"
else
  bad "A7 the suspended-account check is missing from the API-key path"
fi

if has "$AAK_BODY" 'UPDATE api_keys SET last_used_at = NOW\(\) WHERE id = \$1'; then
  ok "A8 last_used_at is updated on successful authentication"
else
  bad "A8 last_used_at bookkeeping is missing"
fi

# ── §B migrations: the hot-path lookup column is indexed ───────────────────
echo
echo "── §B migrations: api_keys.key_hash (the actual WHERE-clause column) is indexed ──"

if [ -n "$MIGRATIONS_GLOB" ]; then
  ok "B0 found the api_keys-hash-index migration file ($(basename "$MIGRATIONS_GLOB"))"
else
  bad "B0 no migration file matching *api_keys_hash*.sql exists"
fi

if has "$MIGRATION_C" 'CREATE UNIQUE INDEX idx_api_keys_hash ON api_keys\(key_hash\);'; then
  ok "B1 a UNIQUE index on api_keys(key_hash) exists — without it, every dp_ request sequential-scans the whole table"
else
  bad "B1 the key_hash index is missing or not UNIQUE"
fi

# ── §C backend_client.rs: pass/fail is decided by the TERMINAL step alone ──
echo
echo "── §C CLI backend_client.rs: follow_progress() and byte-safe SSE framing ──"

if has "$BACKEND_CLIENT_C" 'if token\.is_empty\(\)'; then
  ok "C1 load_token() rejects an empty/whitespace-only token file instead of treating it as configured"
else
  bad "C1 load_token() no longer guards against an empty token file"
fi

FP_BODY=$(fnbody "$BACKEND_CLIENT_C" "follow_progress")

if lacks "$FP_BODY" 'saw_error'; then
  ok "C2 follow_progress() has no cross-step 'saw_error' latch left over from the fixed defect"
else
  bad "C2 a saw_error-shaped latch is back — an earlier advisory-only error step would again fail a successful restore"
fi

# The terminal check must read `status` (this event's own field) at the
# `step_name == "complete"` branch — not a variable accumulated from earlier
# events. Scoped to the tail of the function so an unrelated status==\"error\"
# comparison elsewhere in the file can't satisfy this.
COMPLETE_BRANCH=$(awk '/step_name == "complete"/{f=1} f' <<< "$FP_BODY")
if has "$COMPLETE_BRANCH" 'if status == "error"'; then
  ok "C3 the complete-event branch decides pass/fail from THIS event's own status"
else
  bad "C3 the complete-event branch does not check its own status field"
fi

if has "$BACKEND_CLIENT_C" 'let mut buf: Vec<u8> = Vec::new\(\);' \
  && has "$BACKEND_CLIENT_C" 'String::from_utf8_lossy\(&frame\)' \
  && lacks "$BACKEND_CLIENT_C" 'String::from_utf8_lossy\(&chunk\)'; then
  ok "C4 raw bytes are buffered across chunks and decoded only once a complete frame has arrived (chunk boundaries can split a multi-byte char)"
else
  bad "C4 a raw chunk is decoded independently before a full frame has been assembled — a split multi-byte character would corrupt a printed message"
fi

# ── §D backup.rs: domain resolution matches the restore endpoint's own scope ─
echo
echo "── §D CLI backup.rs: domain→site-id resolution falls back to the admin-sites listing ──"

RESTORE_VIA_BACKEND_BODY=$(fnbody "$BACKUP_C" "cmd_backup_restore_via_backend")

if has "$RESTORE_VIA_BACKEND_BODY" '"/api/admin/sites"'; then
  ok "D1 cmd_backup_restore_via_backend falls back to GET /api/admin/sites"
else
  bad "D1 no /api/admin/sites fallback — an admin's key can restore a site it doesn't own via the API, but the CLI's own domain lookup would 404 first"
fi

if has "$RESTORE_VIA_BACKEND_BODY" 'owned_site_id'; then
  ok "D2 the fallback is conditioned on the owner-scoped lookup coming up empty, not run unconditionally"
else
  bad "D2 no owned/not-owned branch found — cannot confirm the fallback is actually gated"
fi

echo
echo "──────────────────────────────────────────"
echo "PASS: $PASS   FAIL: $FAIL"
[ "$FAIL" -eq 0 ]
