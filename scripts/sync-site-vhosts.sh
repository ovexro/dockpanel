#!/usr/bin/env bash
# Keep the project's own public-surface nginx vhosts under version control.
#
# dockpanel.dev, demo.dockpanel.dev and docs.dockpanel.dev are served straight
# off one box, and until s281 their vhosts existed ONLY on that box: hand-edited
# files with no copy anywhere, no history, and no way to review a change or
# rebuild the machine. Three sessions of cache-header work lived only in them.
#
# A config copied into a repo by hand goes stale, so this is not a copy — it is
# a renderer with a check mode, run by `nginx-contract` in CI and cheap to run
# by hand after any edit:
#
#   scripts/sync-site-vhosts.sh            # check: does the repo match the box?
#   scripts/sync-site-vhosts.sh --pull     # adopt the box's current config
#   scripts/sync-site-vhosts.sh --push     # write the repo's config to the box
#
# HOST-SPECIFIC VALUES ARE PLACEHOLDERS, not because they are secret but because
# this repo is public and neither belongs in it: the origin's LAN address (see
# the :443 note below) and the checkout path the sites are served from.
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="$REPO/deploy/nginx"
MODE="${1:---check}"

# Neither value is hardcoded — writing this box's address into a public repo is
# the exact leak the placeholders exist to prevent, and the pre-commit hook
# blocks on it. Both are derived, so this works unchanged on any origin.
detect_origin_ip() {
    [ -n "${ORIGIN_IP:-}" ] && { printf '%s' "$ORIGIN_IP"; return; }
    local f ip
    # Whatever the live vhosts already bind is by definition the right answer.
    for f in /etc/nginx/sites-enabled/dockpanel.dev.conf \
             /etc/nginx/sites-available/docs.dockpanel.dev.conf; do
        [ -r "$f" ] || continue
        ip=$(grep -m1 -oE '^[[:space:]]*listen[[:space:]]+([0-9]{1,3}\.){3}[0-9]{1,3}:' "$f" \
             | grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}')
        [ -n "$ip" ] && { printf '%s' "$ip"; return; }
    done
    # Fresh box with no vhost yet: the address it reaches the network from.
    ip route get 1.1.1.1 2>/dev/null \
      | grep -oE 'src ([0-9]{1,3}\.){3}[0-9]{1,3}' | awk '{print $2}'
}

ORIGIN_IP="$(detect_origin_ip)"
SITE_ROOT="${SITE_ROOT:-$REPO}"   # the three sites are served from this checkout

# live path → repo filename
FILES="
/etc/nginx/sites-enabled/dockpanel.dev.conf:dockpanel.dev.conf
/etc/nginx/sites-available/docs.dockpanel.dev.conf:docs.dockpanel.dev.conf
"

redact()  { sed -e "s#$ORIGIN_IP#__ORIGIN_IP__#g" -e "s#$SITE_ROOT#__SITE_ROOT__#g"; }
restore() { sed -e "s#__ORIGIN_IP__#$ORIGIN_IP#g" -e "s#__SITE_ROOT__#$SITE_ROOT#g"; }

mkdir -p "$DEST"
RC=0

for pair in $FILES; do
    live="${pair%%:*}"; name="${pair##*:}"; repo="$DEST/$name"

    case "$MODE" in
      --pull)
        [ -r "$live" ] || { echo "skip: $live not readable here"; continue; }
        redact < "$live" > "$repo"
        echo "pulled  $live → deploy/nginx/$name"
        ;;
      --push)
        [ -r "$repo" ] || { echo "missing: deploy/nginx/$name"; RC=1; continue; }
        restore < "$repo" > "$live"
        echo "pushed  deploy/nginx/$name → $live"
        ;;
      --check)
        if [ ! -r "$repo" ]; then
            echo "MISSING: deploy/nginx/$name is not in the repo"; RC=1; continue
        fi
        # Off-box (CI) there is nothing to compare against; assert the copy is
        # present and carries no host-specific value instead. A check that
        # silently passes when it cannot see the thing it checks is worse than
        # no check — this is the same vacuous-pass class as lesson #108d.
        if [ ! -r "$live" ]; then
            if grep -qE "$ORIGIN_IP|$SITE_ROOT" "$repo"; then
                echo "LEAK: deploy/nginx/$name contains a host-specific value"; RC=1
            else
                echo "ok (repo-only): deploy/nginx/$name present, no host values"
            fi
            continue
        fi
        if redact < "$live" | diff -q - "$repo" > /dev/null 2>&1; then
            echo "ok: deploy/nginx/$name matches the box"
        else
            echo "DRIFT: deploy/nginx/$name differs from $live"
            redact < "$live" | diff - "$repo" | sed 's/^/    /'
            RC=1
        fi
        ;;
      *)
        echo "usage: $0 [--check|--pull|--push]"; exit 2 ;;
    esac
done

[ "$RC" -eq 0 ] || echo "
Run '$0 --pull' to adopt the box's config, or '--push' to overwrite the box."
exit "$RC"
