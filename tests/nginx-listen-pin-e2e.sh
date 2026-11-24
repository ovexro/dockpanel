#!/usr/bin/env bash
# Regression pins for the s257 fresh-box audit (v2.31.2).
#
# These do NOT need a running panel: they exercise the exact match/rewrite
# shapes the install and update scripts rely on, against fixtures that
# reproduce what certbot and setup.sh actually write. Every case below failed
# before v2.31.2.
#
#   run: bash tests/nginx-listen-pin-e2e.sh
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SETUP="$REPO/scripts/setup.sh"
UPDATE="$REPO/scripts/update.sh"
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected [$3], got [$2])"; fi; }

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

echo "── F1: the BASE_URL repair must not be shadowed by DATABASE_URL ──"
# setup.sh writes DATABASE_URL as the FIRST line of every api.env, and that
# string CONTAINS "BASE_URL" — an unanchored grep matched on every install
# that has ever existed, so the repair below it never ran once.
cat > "$TMP/api.env" <<'EOF'
DATABASE_URL=postgresql://dockpanel:pw@127.0.0.1:5450/dockpanel
JWT_SECRET=x
LISTEN_ADDR=127.0.0.1:3080
EOF
if grep -qE '^BASE_URL=.+' "$TMP/api.env"; then R=set; else R=unset; fi
check "an api.env with only DATABASE_URL reads as BASE_URL-unset" "$R" "unset"

printf 'BASE_URL=\n' >> "$TMP/api.env"
if grep -qE '^BASE_URL=.+' "$TMP/api.env"; then R=set; else R=unset; fi
check "a bare 'BASE_URL=' (domainless install) still reads as unset" "$R" "unset"

printf 's|^BASE_URL=.*|BASE_URL=https://panel.example.com|\n' >/dev/null
sed -i -E "s|^BASE_URL=.*|BASE_URL=https://panel.example.com|" "$TMP/api.env"
check "repairing rewrites in place (never two BASE_URL keys)" \
      "$(grep -c '^BASE_URL=' "$TMP/api.env")" "1"
if grep -qE '^BASE_URL=.+' "$TMP/api.env"; then R=set; else R=unset; fi
check "after repair it reads as set" "$R" "set"

check "update.sh no longer uses the unanchored guard" \
      "$(grep -c 'grep -q "BASE_URL"' "$UPDATE")" "0"

echo
echo "── F3/F5: the panel's :443 must not be a wildcard beside explicit-IP sites ──"
# certbot --nginx writes a wildcard `listen 443 ssl;` while agent site vhosts
# bind `<ip>:443 ssl`. nginx makes those separate sockets, the explicit-IP one
# wins, and the first site with a cert takes over the panel's own hostname.
cat > "$TMP/panel.conf" <<'EOF'
server {
    server_name panel.example.com;
    listen [::]:443 ssl ipv6only=on; # managed by Certbot
    listen 443 ssl; # managed by Certbot
    ssl_certificate /etc/letsencrypt/live/panel.example.com/fullchain.pem; # managed by Certbot
}
EOF
# The old repair's REWRITE targeted `listen <something>:443 ssl;` — it needs a
# colon before the port, so certbot's `listen 443 ssl;` slipped past it. (Its
# grep did fire, but only on the [::]:443 line, which is why this went unseen.)
cp "$TMP/panel.conf" "$TMP/old.conf"
sed -i -E '0,/^([[:space:]]*)listen ([^;]*):443 ssl;[[:space:]]*$/{s||\1listen \2:443 ssl;\n\1listen [::]:443 ssl;|}' "$TMP/old.conf"
check "the OLD rewrite leaves the wildcard :443 untouched" \
      "$(grep -cE '^[[:space:]]*listen 443 ssl;' "$TMP/old.conf")" "1"

if grep -qE '^[[:space:]]*listen 443 ssl;' "$TMP/panel.conf"; then R=matched; else R=missed; fi
check "the NEW guard does see it" "$R" "matched"

sed -i -E "s|^([[:space:]]*)listen 443 ssl;|\1listen 10.0.0.5:443 ssl;|" "$TMP/panel.conf"
sed -i -E 's|^([[:space:]]*)listen \[::\]:443 ssl ipv6only=on;|\1listen [::]:443 ssl;|' "$TMP/panel.conf"
check "wildcard :443 is pinned to the interface IP" \
      "$(grep -c 'listen 10.0.0.5:443 ssl;' "$TMP/panel.conf")" "1"
check "no wildcard ':443 ssl' remains" \
      "$(grep -cE '^[[:space:]]*listen 443 ssl;' "$TMP/panel.conf")" "0"
check "ipv6only=on is stripped (sites use a plain [::]:443)" \
      "$(grep -c 'ipv6only=on' "$TMP/panel.conf")" "0"
check "the certbot trailing comment survives the rewrite" \
      "$(grep -c 'listen 10.0.0.5:443 ssl; # managed by Certbot' "$TMP/panel.conf")" "1"

echo
echo "── the scripts actually wire it up ──"
check "setup.sh defines normalize_panel_listen"  "$(grep -c '^normalize_panel_listen()' "$SETUP")" "1"
check "setup.sh calls it after certbot succeeds" "$(grep -c '^        normalize_panel_listen$' "$SETUP")" "1"

# F4: a reload cannot move an already-bound 0.0.0.0:443 listener to <ip>:443 —
# nginx inherits the old socket and the rewrite silently no-ops. Proven on a
# live box: reload left the panel broken, restart fixed it.
if grep -q 'systemctl restart nginx' "$SETUP"; then R=yes; else R=no; fi
check "setup.sh RESTARTS nginx (a reload would no-op the rebind)" "$R" "yes"
if grep -q 'NGINX_NEEDS_RESTART' "$UPDATE"; then R=yes; else R=no; fi
check "update.sh restarts too, for boxes already broken" "$R" "yes"

echo
echo "── F6: automatic renewal must resolve the ACME contact like issuance does ──"
SS="$REPO/panel/backend/src/services/security_scanner.rs"
AH="$REPO/panel/backend/src/services/auto_healer.rs"
# Two doors: the site renewal, and (since #138) the Docker-app arm for an adopted
# row that carries no contact of its own.
check "security_scanner renewal doors use resolve_acme_contact" "$(grep -c 'resolve_acme_contact' "$SS")" "2"
check "auto_healer renewal uses resolve_acme_contact"      "$(grep -c 'resolve_acme_contact' "$AH")" "1"

echo
echo "── F7: unmatched hosts must never reach the panel (#136) ──"
# Both scripts write the same catch-all. Run setup.sh's REAL function against a
# scratch dir (its path and `nginx -t` redirected), then compare update.sh's copy.
FN=$(awk '/^write_default_catchall\(\) \{/{f=1} f{print} f&&/^CATCHEOF$/{d=1} f&&d&&/^\}$/{exit}' "$SETUP" \
     | sed "s|/etc/nginx/conf.d|$TMP/conf.d|g")
check "setup.sh defines write_default_catchall" "$(printf '%s' "$FN" | grep -c '^write_default_catchall()')" "1"
( nginx() { return 0; }; warn() { :; }; eval "$FN"; write_default_catchall 203.0.113.7 )
C="$TMP/conf.d/00-dockpanel-default.conf"
check "port 80 drops unmatched hosts on the interface IP and on IPv6" \
  "$(grep -cE 'listen (203\.0\.113\.7:80|\[::\]:80) default_server;|return 444;' "$C")" "3"
check "port 443 refuses the TLS handshake instead of serving a certificate" \
  "$(grep -cE 'listen (203\.0\.113\.7:443|\[::\]:443) ssl default_server;|ssl_reject_handshake on;' "$C")" "3"
( nginx() { return 0; }; warn() { :; }; eval "$FN"; write_default_catchall "" )
check "without an interface IP it falls back to the wildcard sockets" \
  "$(grep -cE 'listen (80|443 ssl) default_server;' "$C")" "2"
( nginx() { return 1; }; warn() { :; }; eval "$FN"; write_default_catchall 203.0.113.7 )
check "a catch-all nginx rejects is removed, never left behind" "$([ -e "$C" ] && echo kept || echo gone)" "gone"
check "configure_nginx calls it only in domain mode" \
  "$(grep -B1 'write_default_catchall "\${BIND_IP:-}"' "$SETUP" | grep -c 'if \[ -n "\$PANEL_DOMAIN" \]')" "1"
check "update.sh writes the same two server blocks" \
  "$(grep -cE '^[[:space:]]+(return 444;|ssl_reject_handshake on;|listen \[::\]:(80|443)( ssl)? default_server;)$' "$UPDATE")" "4"
check "update.sh leaves an existing file alone and skips a non-domain panel" \
  "$(grep -cE '\[ ! -f "\$CATCH_CONF" \]|\[ "\$CATCH_DOMAIN" != "_" \]' "$UPDATE")" "2"

echo
echo "══ $PASS passed, $FAIL failed ══"
[ "$FAIL" -eq 0 ]
