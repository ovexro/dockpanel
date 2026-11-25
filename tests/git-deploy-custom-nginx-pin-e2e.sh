#!/usr/bin/env bash
# Regression pin for #132 (part 2): extra nginx directives + upload limit per
# Git Deploy domain. The vhost is rewritten from many places (cert renewal, TLS
# switch, recreate, rollback) and a blue-green update only swaps the port, so the
# values live in a per-domain sidecar the renderer falls back to, and are patched
# into the live vhost on Save and after every deploy.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
PASS=0; FAIL=0
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; PASS=$((PASS+1)); }
bad() { printf '\033[31m✗\033[0m %s — %s\n' "$1" "$2"; FAIL=$((FAIL+1)); }
has() { /usr/bin/grep -qE -- "$3" "$2" && ok "$1" || bad "$1" "pattern '$3' not in $2"; }

NG=panel/agent/src/services/nginx.rs
RT=panel/agent/src/routes/git_build.rs
GB=panel/agent/src/services/git_build.rs
GD=panel/backend/src/routes/git_deploys.rs
UI=panel/frontend/src/pages/GitDeploys.tsx
MIG=$(ls panel/backend/migrations/*git_deploy_custom_nginx.sql 2>/dev/null | head -1)

[ -n "$MIG" ] && ok "migration exists" || bad "migration exists" "none"
has "migration adds custom_nginx" "${MIG:-/dev/null}" 'custom_nginx TEXT'
has "migration adds max_upload_mb" "${MIG:-/dev/null}" 'max_upload_mb INT'
for t in proxy https http; do
  has "template $t closes the custom block with a marker" "panel/agent/src/templates/nginx/$t.conf" '# End custom directives'
done
has "renderer falls back to the stored extras" "$NG" 'load_extras\(domain\)'
has "stored directives pass the same validation" "$NG" 'validate_custom_nginx\(c\)\?'
has "vhost is patched in place, nginx -t, restore on failure" "$NG" 'restore_or_remove\(&config_path, Some\(&previous\)\)'
has "agent /git/deploy stores the extras" "$RT" 'save_extras\(d, extras\)'
has "agent applies them after a (blue-green) deploy" "$RT" 'apply_vhost_extras\(d\)'
has "agent exposes /git/vhost-extras" "$RT" '"/git/vhost-extras", post\(vhost_extras\)'
has "a refused apply restores the stored values" "$RT" 'save_extras\(&body\.domain, before\)'
has "extras go when the vhost does" "$GB" 'remove_extras\(domain\)'
has "backend validates on create" "$GD" 'validate_custom_nginx\(body\.custom_nginx'
has "backend asks the agent BEFORE saving" "$GD" '"/git/vhost-extras"'
has "update writes custom_nginx (\"\" clears)" "$GD" "custom_nginx = CASE WHEN \\\$23 = ''"
has "update writes max_upload_mb (0 clears)" "$GD" 'max_upload_mb = CASE WHEN \$24 = 0'
has "every deploy body carries them" "$GD" 'custom_nginx: config\.custom_nginx\.as_deref\(\)'
has "UI sends both fields" "$UI" 'custom_nginx: formCustomNginx\.trim\(\)'
has "UI loads the stored values" "$UI" 'setFormMaxUpload\(selected\.max_upload_mb'

echo
[ "$FAIL" -eq 0 ] && printf '\033[32mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL" || printf '\033[31mPASS %d  FAIL %d\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
