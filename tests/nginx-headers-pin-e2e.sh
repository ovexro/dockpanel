#!/usr/bin/env bash
# Regression pins for the nginx response contract (s281, v2.47.2).
#
# WHY THIS EXISTS
# ---------------
# s278 found index.html served with no cache directive at all, so every deploy
# was invisible to returning visitors. s280 gave that fix a delivery path. Both
# shipped WITHOUT a pin, and this project's own history says a fix without a pin
# regresses — so the whole contract is asserted here, on both surfaces that can
# produce it: the template setup.sh writes at INSTALL time, and the migration
# update.sh applies to a box that installed BEFORE the fix existed.
#
# THE TRAP THIS SUITE IS BUILT AROUND (lesson #108d)
# --------------------------------------------------
# The s280 harness returned 403 for every request and ELEVEN OF ITS TWELVE
# header assertions still passed: these headers are declared `always`, so nginx
# emits them on error pages too. The single honest failure was /assets/, whose
# cache header is deliberately not `always` and correctly vanished on the 4xx.
# One real failure exposed eleven vacuous passes.
#
#   >>> Every serving assertion below therefore proves HTTP 200 FIRST. <<<
#
#   run: bash tests/nginx-headers-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SETUP="$REPO/scripts/setup.sh"
UPDATE="$REPO/scripts/update.sh"
PASS=0; FAIL=0; SKIP=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
skip() { SKIP=$((SKIP+1)); printf '  \033[0;33m—\033[0m %s (skipped)\n' "$1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected [$3], got [$2])"; fi; }

TMP=$(mktemp -d); trap 'rm -rf "$TMP"; [ -n "${NGX_PID:-}" ] && kill "$NGX_PID" 2>/dev/null' EXIT
NGX_PID=""

# The seven headers setup.sh declares at server level. Named once here so a
# future header added to the template without being repeated in a location
# block fails §1 rather than going unnoticed.
PANEL_HEADERS="X-Content-Type-Options X-Frame-Options Referrer-Policy Permissions-Policy Strict-Transport-Security Content-Security-Policy X-XSS-Protection"

# ─────────────────────────────────────────────────────────────────────────────
echo "── 1: the template setup.sh writes (reaches boxes installed AFTER the fix) ──"
# ─────────────────────────────────────────────────────────────────────────────
# Extract the panel vhost heredoc verbatim rather than re-describing it, so the
# assertions cannot drift from what actually gets written.
sed -n '/cat > "\$NGINX_CONF" << NGINXEOF/,/^NGINXEOF$/p' "$SETUP" \
  | sed '1d;$d' > "$TMP/template.conf"

check "the panel vhost template was extracted" \
      "$([ -s "$TMP/template.conf" ] && echo yes || echo no)" "yes"

# The block-slicing helper: print one location block by its opening line.
block() { awk -v pat="$1" '$0 ~ pat {f=1} f {print} f && /^    [}]/ {exit}' "$2"; }

ASSETS=$(block '^    location /assets/ [{]' "$TMP/template.conf")
INDEXH=$(block '^    location = /index[.]html [{]' "$TMP/template.conf")

for h in $PANEL_HEADERS; do
    check "/assets/ repeats $h" \
          "$(printf '%s' "$ASSETS" | grep -c "add_header $h ")" "1"
done

check "/assets/ sets exactly one Cache-Control" \
      "$(printf '%s' "$ASSETS" | grep -c 'add_header Cache-Control')" "1"
check "/assets/ no longer uses 'expires' (it emits a SECOND Cache-Control)" \
      "$(printf '%s' "$ASSETS" | grep -c 'expires ')" "0"
check "/assets/ Cache-Control is immutable with a max-age" \
      "$(printf '%s' "$ASSETS" | grep -c 'public, max-age=31536000, immutable')" "1"
# Not `always`: a 404 during an update must not be cached for a year.
check "/assets/ Cache-Control is NOT 'always' (an error must not be cached)" \
      "$(printf '%s' "$ASSETS" | grep -c 'add_header Cache-Control "public, max-age=31536000, immutable";$')" "1"

for h in $PANEL_HEADERS; do
    check "= /index.html repeats $h" \
          "$(printf '%s' "$INDEXH" | grep -c "add_header $h ")" "1"
done
check "= /index.html is no-cache" \
      "$(printf '%s' "$INDEXH" | grep -c 'add_header Cache-Control "no-cache" always;')" "1"

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "── 2: the other two shipped templates repeat their OWN parent set ──"
# ─────────────────────────────────────────────────────────────────────────────
# The parent sets differ per file (the panel vhost declares seven, these two
# declare five and four), so "N of 7" is the wrong frame — each /assets/ block
# is measured against the server block it actually sits in.
for f in panel/frontend/nginx.conf website/client/nginx.conf; do
    CONF="$REPO/$f"
    PARENT=$(grep -oE '^    add_header [A-Za-z-]+' "$CONF" | awk '{print $2}' | sort -u)
    A=$(block '^    location /assets/ [{]' "$CONF")
    MISSING=0
    for h in $PARENT; do
        printf '%s' "$A" | grep -c "add_header $h " >/dev/null || MISSING=$((MISSING+1))
    done
    check "$f: /assets/ repeats all $(printf '%s' "$PARENT" | wc -w) of its parent headers" "$MISSING" "0"
    check "$f: /assets/ sets exactly one Cache-Control" \
          "$(printf '%s' "$A" | grep -c 'add_header Cache-Control')" "1"
    check "$f: /assets/ no longer uses 'expires'" \
          "$(printf '%s' "$A" | grep -c 'expires ')" "0"
done

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "── 3: the update.sh migrations (reach boxes installed BEFORE the fix) ──"
# ─────────────────────────────────────────────────────────────────────────────
# A fix in the install template reaches nobody already running — update.sh never
# re-runs setup.sh (lesson #108). These assert the two migrations that close it.
check "update.sh carries the index.html cache migration" \
      "$(grep -c 'Added index.html cache directive' "$UPDATE")" "1"
check "update.sh carries the /assets/ header migration" \
      "$(grep -c 'Repeated the security headers on /assets/' "$UPDATE")" "1"
check "both migrations refuse a vhost with no server-level add_header" \
      "$(grep -c 'has no server-level add_header' "$UPDATE")" "2"

# A pre-fix vhost, as a box installed on v2.47.0 actually has it. Its header set
# is deliberately NOT today's — three headers, one of which today's template
# does not even declare — so a migration that writes the script's list instead
# of copying the box's own fails loudly here.
cat > "$TMP/old-panel.conf" <<'EOF'
server {
    listen 8080;
    server_name _;
    root /var/www/panel;
    index index.html;

    location / {
        try_files $uri $uri/ /index.html;
    }

    location /assets/ {
        expires 1y;
        add_header Cache-Control "public, immutable";
    }

    server_tokens off;

    add_header X-Content-Type-Options "nosniff" always;
    add_header X-Frame-Options "SAMEORIGIN" always;
    add_header Content-Security-Policy "default-src 'self'; frame-ancestors 'none';" always;
}
EOF
cp "$TMP/old-panel.conf" "$TMP/migrated.conf"

# Run both migrations exactly as update.sh defines them, against the fixture.
run_migrations() {
    local target="$1" out
    out=$(
        set +u
        log() { :; }
        NGINX_NEEDS_RELOAD=0
        # shellcheck disable=SC1090
        . <(sed -n '/^# ── v2.47.1: give index.html a cache directive/,/^done$/p' "$UPDATE" \
              | sed "s#/etc/nginx/sites-enabled/dockpanel-panel.conf /etc/nginx/conf.d/dockpanel-panel.conf#$target#")
        # shellcheck disable=SC1090
        . <(sed -n '/^# ── v2.47.2: repeat the security headers on \/assets\//,/^done$/p' "$UPDATE" \
              | sed "s#/etc/nginx/sites-enabled/dockpanel-panel.conf /etc/nginx/conf.d/dockpanel-panel.conf#$target#")
        echo "$NGINX_NEEDS_RELOAD"
    )
    echo "$out"
}

R=$(run_migrations "$TMP/migrated.conf")
check "migrating a v2.47.0 vhost signals a reload" "$R" "1"

MA=$(block '^    location /assets/ [{]' "$TMP/migrated.conf")
MI=$(block '^    location = /index[.]html [{]' "$TMP/migrated.conf")

# THE POINT OF #108c: the block must carry the BOX's headers, not the script's.
check "migrated /assets/ copied the box's own X-Frame-Options (SAMEORIGIN)" \
      "$(printf '%s' "$MA" | grep -c 'X-Frame-Options "SAMEORIGIN"')" "1"
check "migrated /assets/ did NOT import today's DENY from the script" \
      "$(printf '%s' "$MA" | grep -c 'X-Frame-Options "DENY"')" "0"
check "migrated /assets/ did NOT invent headers the box never declared" \
      "$(printf '%s' "$MA" | grep -c 'Strict-Transport-Security\|Permissions-Policy\|X-XSS-Protection')" "0"
check "migrated /assets/ carries the box's CSP" \
      "$(printf '%s' "$MA" | grep -c 'Content-Security-Policy')" "1"
check "migrated /assets/ sets exactly one Cache-Control" \
      "$(printf '%s' "$MA" | grep -c 'add_header Cache-Control')" "1"
check "migrated /assets/ dropped 'expires'" \
      "$(printf '%s' "$MA" | grep -c 'expires ')" "0"
check "migrated = /index.html copied the box's own SAMEORIGIN too" \
      "$(printf '%s' "$MI" | grep -c 'X-Frame-Options "SAMEORIGIN"')" "1"

# Idempotence — update.sh runs on EVERY update, not once.
cp "$TMP/migrated.conf" "$TMP/migrated.once"
R=$(run_migrations "$TMP/migrated.conf")
check "re-running both migrations signals no reload" "$R" "0"
check "re-running both migrations is byte-identical" \
      "$(cmp -s "$TMP/migrated.once" "$TMP/migrated.conf" && echo same || echo changed)" "same"

# The OTHER live population: a box on v2.47.1, which already has the index.html
# block (from the template or the seventh migration) but still has the old
# /assets/. The eighth migration must fix /assets/ and leave the existing block
# alone — not skip, not duplicate.
cat > "$TMP/v471.conf" <<'EOF'
server {
    listen 8080;
    root /var/www/panel;
    index index.html;

    location / {
        try_files $uri $uri/ /index.html;
    }

    location = /index.html {
        add_header Cache-Control "no-cache" always;
        add_header X-Content-Type-Options "nosniff" always;
        add_header X-Frame-Options "DENY" always;
    }

    location /assets/ {
        expires 1y;
        add_header Cache-Control "public, immutable";
    }

    add_header X-Content-Type-Options "nosniff" always;
    add_header X-Frame-Options "DENY" always;
}
EOF
R=$(run_migrations "$TMP/v471.conf")
check "a v2.47.1 box still gets the /assets/ fix" "$R" "1"
check "  and keeps exactly one index.html block (not duplicated)" \
      "$(grep -c 'location = /index.html' "$TMP/v471.conf")" "1"
check "  and exactly one /assets/ block" \
      "$(grep -c 'location /assets/' "$TMP/v471.conf")" "1"
check "  and its /assets/ now carries nosniff" \
      "$(block '^    location /assets/ [{]' "$TMP/v471.conf" | grep -c 'X-Content-Type-Options')" "1"
cp "$TMP/v471.conf" "$TMP/v471.once"
R=$(run_migrations "$TMP/v471.conf")
check "  and re-running is a byte-identical no-op" \
      "$(cmp -s "$TMP/v471.once" "$TMP/v471.conf" && echo same || echo changed)$R" "same0"

# A vhost with `expires` in some OTHER location must still migrate. The
# migration's own post-condition originally grepped the whole file for
# `expires`, which would have skipped this box silently — a fix that reaches
# nobody, which is the failure this suite exists to prevent.
cat > "$TMP/otherexpires.conf" <<'EOF'
server {
    listen 8080;
    root /var/www/panel;

    location /media/ {
        expires 1y;
        add_header Cache-Control "public";
    }

    location /assets/ {
        expires 1y;
        add_header Cache-Control "public, immutable";
    }

    add_header X-Content-Type-Options "nosniff" always;
}
EOF
R=$(run_migrations "$TMP/otherexpires.conf")
check "a vhost with 'expires' in another location still migrates" "$R" "1"
check "  /assets/ was rewritten" \
      "$(block '^    location /assets/ [{]' "$TMP/otherexpires.conf" | grep -c 'max-age=31536000')" "1"
check "  the unrelated /media/ block was left alone" \
      "$(block '^    location /media/ [{]' "$TMP/otherexpires.conf" | grep -c 'expires 1y')" "1"

# A vhost with no server-level add_header is skipped, not guessed at.
cat > "$TMP/headerless.conf" <<'EOF'
server {
    listen 8080;
    root /var/www/panel;
    location / {
        try_files $uri $uri/ /index.html;
    }
    location /assets/ {
        expires 1y;
        add_header Cache-Control "public, immutable";
    }
}
EOF
cp "$TMP/headerless.conf" "$TMP/headerless.orig"
R=$(run_migrations "$TMP/headerless.conf")
check "a header-less vhost signals no reload" "$R" "0"
check "a header-less vhost is left byte-identical" \
      "$(cmp -s "$TMP/headerless.orig" "$TMP/headerless.conf" && echo same || echo changed)" "same"

# ─────────────────────────────────────────────────────────────────────────────
echo
echo "── 4: served by a real nginx — status FIRST, then headers (#108d) ──"
# ─────────────────────────────────────────────────────────────────────────────
if ! command -v nginx > /dev/null 2>&1; then
    skip "real-nginx serving assertions (nginx not installed)"
else
    PORT=8${RANDOM:0:3}; [ "$PORT" -lt 8100 ] && PORT=8451
    mkdir -p "$TMP/www/assets" "$TMP/ngx/logs" "$TMP/ngx/tmp"
    echo '<!doctype html><title>panel</title><script src="/assets/app-abc123.js"></script>' > "$TMP/www/index.html"
    echo 'console.log(1)' > "$TMP/www/assets/app-abc123.js"

    # Serve the REAL template, with its install-time variables filled in.
    sed -e "s#\${LISTEN_DIRECTIVE}#listen 127.0.0.1:$PORT;#" \
        -e "s#\${SERVER_NAME}#localhost#" \
        -e "s#\${PANEL_TLS_BLOCK}##" \
        -e "s#\${FE_ROOT}#$TMP/www#" \
        -e "s#\\\\\$#\$#g" \
        -e "s#include /etc/nginx/conf.d/dockpanel-panel.locations/\*.conf;##" \
        "$TMP/template.conf" > "$TMP/ngx/server.conf"

    # When this runs as root, nginx would drop its workers to `nobody`, which
    # cannot traverse a 0700 mktemp dir — every request comes back 403. That is
    # exactly the s280 failure this suite is built to detect, so it must not be
    # allowed to happen for a reason as uninteresting as file ownership.
    cat > "$TMP/ngx/nginx.conf" <<EOF
worker_processes 1;
$([ "$(id -u)" = "0" ] && echo "user root;")
pid $TMP/ngx/nginx.pid;
error_log $TMP/ngx/logs/error.log;
events { worker_connections 64; }
http {
    include $(nginx -V 2>&1 | grep -oE '\-\-conf-path=[^ ]+' | cut -d= -f2 | xargs dirname 2>/dev/null || echo /etc/nginx)/mime.types;
    default_type application/octet-stream;
    access_log $TMP/ngx/logs/access.log;
    client_body_temp_path $TMP/ngx/tmp;
    proxy_temp_path $TMP/ngx/tmp;
    fastcgi_temp_path $TMP/ngx/tmp;
    uwsgi_temp_path $TMP/ngx/tmp;
    scgi_temp_path $TMP/ngx/tmp;
    include $TMP/ngx/server.conf;
}
EOF

    if ! nginx -t -c "$TMP/ngx/nginx.conf" > "$TMP/ngx/t.log" 2>&1; then
        bad "the generated panel vhost passes nginx -t"
        sed 's/^/      /' "$TMP/ngx/t.log"
    else
        ok "the generated panel vhost passes nginx -t"
        nginx -c "$TMP/ngx/nginx.conf" > /dev/null 2>&1
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            curl -sf -o /dev/null "http://127.0.0.1:$PORT/" 2>/dev/null && break
            sleep 0.3
        done
        NGX_PID=$(cat "$TMP/ngx/nginx.pid" 2>/dev/null || true)

        # hdrs <path> → status on line 1, headers after. EVERY assertion below
        # gates on that status, because `always` headers survive an error page.
        hdrs() { curl -sI --max-time 5 "http://127.0.0.1:$PORT$1" 2>/dev/null; }
        status() { printf '%s' "$1" | head -1 | awk '{print $2}'; }
        hval() { printf '%s' "$1" | grep -i "^$2:" | sed "s/^[^:]*: *//" | tr -d '\r'; }
        hcount() { printf '%s' "$1" | grep -ci "^$2:"; }

        # -- the document --
        D=$(hdrs /index.html)
        check "GET /index.html is 200 (asserted BEFORE any header is read)" "$(status "$D")" "200"
        if [ "$(status "$D")" = "200" ]; then
            check "  /index.html is no-cache" "$(hval "$D" Cache-Control)" "no-cache"
            for h in $PANEL_HEADERS; do
                check "  /index.html carries $h" "$(hcount "$D" "$h")" "1"
            done
        fi

        # -- the SPA root and a deep route both resolve to the same document --
        for p in / /sites/42/settings; do
            R=$(hdrs "$p")
            check "GET $p is 200" "$(status "$R")" "200"
            [ "$(status "$R")" = "200" ] && \
              check "  $p is no-cache (SPA fallback keeps the directive)" "$(hval "$R" Cache-Control)" "no-cache"
        done

        # -- the asset: the regression that started this session --
        A=$(hdrs /assets/app-abc123.js)
        check "GET /assets/app-abc123.js is 200" "$(status "$A")" "200"
        if [ "$(status "$A")" = "200" ]; then
            check "  the asset carries X-Content-Type-Options (it carried NONE before)" \
                  "$(hval "$A" X-Content-Type-Options)" "nosniff"
            for h in $PANEL_HEADERS; do
                check "  the asset carries $h" "$(hcount "$A" "$h")" "1"
            done
            check "  the asset has exactly ONE Cache-Control line (it had two)" \
                  "$(hcount "$A" Cache-Control)" "1"
            check "  the asset is immutable" \
                  "$(hval "$A" Cache-Control)" "public, max-age=31536000, immutable"
            check "  the asset sends no Expires (it contradicted Cache-Control)" \
                  "$(hcount "$A" Expires)" "0"
        fi

        # -- #108d itself, as an assertion: prove the suite can still fail --
        E=$(hdrs /assets/does-not-exist.js)
        check "a missing asset is 404 (the harness can distinguish error from success)" \
              "$(status "$E")" "404"
        # The security headers ARE emitted on the 404 — that is what made eleven
        # assertions vacuous at s280. The cache header is not, by design. These
        # two are themselves gated on the status: asserted against a 403 they
        # would both pass while proving nothing, which is the whole lesson.
        if [ "$(status "$E")" = "404" ]; then
            check "  the 404 still carries nosniff ('always' — this is why status must be checked)" \
                  "$(hval "$E" X-Content-Type-Options)" "nosniff"
            check "  the 404 carries NO Cache-Control (an error must never be cached)" \
                  "$(hcount "$E" Cache-Control)" "0"
        fi

        [ -n "$NGX_PID" ] && kill "$NGX_PID" 2>/dev/null; NGX_PID=""
    fi
fi

echo
echo "──────────────────────────────────────────"
printf 'PASS %d   FAIL %d   SKIP %d\n' "$PASS" "$FAIL" "$SKIP"
[ "$FAIL" -eq 0 ] || exit 1
