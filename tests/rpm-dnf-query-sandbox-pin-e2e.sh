#!/usr/bin/env bash
#
# Regression pin: a dnf/yum QUERY made from inside the agent sandbox must not
# use the system metadata cache.
#
# The agent runs under ProtectSystem=strict and /var/cache/dnf is not in its
# ReadWritePaths. dnf takes /var/cache/dnf/metadata_lock.pid even for a
# read-only query, so every sandboxed `list`, `repoquery` and `module list`
# failed with
#
#   [Errno 30] Read-only file system: '/var/cache/dnf/metadata_lock.pid'
#
# Measured on AlmaLinux 9.9 and 10.3, warm and expired cache alike, by running
# the queries under systemd-run with the unit's own sandbox properties; `-C`
# fails the same way because the lock is taken first. Every caller read the
# failure as "not available". Driven on AlmaLinux 9.9 under the real unit:
# Settings -> Services -> PHP answered "PHP 8.3 with FPM installed" and put
# PHP 8.0.30 (EOL) on the box with no extensions, because php_streams() saw no
# streams and install_available() saw no candidates; the PHP version picker
# failed outright with "Unsupported distro for PHP install: ID='almalinux'".
# The fix gives queries their own cache inside /var/lib/dockpanel, which the
# agent may already write. The same two actions then installed PHP 8.4.28 and
# PHP 8.2 from their module streams, each with its extensions.
#
# HOW THE SUBJECTS ARE FOUND. Not from a list of functions: every line in the
# agent that names a dnf query verb is a subject, and the five lines ending at
# it (an args array rustfmt splits over several lines) must carry the option.
# A query added tomorrow is covered the day it is written.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

UNIT=panel/agent/dockpanel-agent.service
PKG=panel/agent/src/services/pkg.rs
AGENT_SRC=panel/agent/src
for f in "$UNIT" "$PKG"; do
  [ -f "$f" ] || { echo "missing $f"; exit 1; }
done

# Code only: the test module is cut at its cfg line and // comments are blanked,
# so prose that quotes a query verb cannot become a subject or satisfy an arm.
# Portable ERE boundaries, not \< \> (gawk-only; mawk reads them literally).
code_of() {
  awk '/^#\[cfg\((.*[^A-Za-z0-9_])?test([^A-Za-z0-9_].*)?\)\]/ { exit }
       { sub(/\/\/.*$/, ""); print }' "$1"
}

# Prints "HIT file:line" for every dnf query site and "MISS file:line" for each
# one whose window does not pass DNF_QUERY_CACHEDIR. A function, so §3 can point
# it at a fixture holding the real defect.
scan() {
  local f
  while IFS= read -r f; do
    code_of "$f" | awk -v F="$f" '
      { l[NR] = $0 }
      END {
        for (i = 1; i <= NR; i++) {
          if (l[i] !~ /"(repoquery|--available|--installed)"|"module", *"list"/) continue
          w = ""
          for (j = (i > 4 ? i - 4 : 1); j <= i; j++) w = w l[j]
          print "HIT " F ":" i
          if (w !~ /DNF_QUERY_CACHEDIR/) print "MISS " F ":" i
        }
      }'
  done < <(find "$1" -name '*.rs' | sort)
}

echo "§1 the query cache is somewhere the sandbox lets the agent write"

CACHEDIR=$(code_of "$PKG" | sed -n 's/.*DNF_QUERY_CACHEDIR: &str = "--setopt=cachedir=\([^"]*\)".*/\1/p' | head -1)
if [ -n "$CACHEDIR" ]; then
  ok "DNF_QUERY_CACHEDIR is defined ($CACHEDIR)"
else
  bad "DNF_QUERY_CACHEDIR is missing from $PKG, or no longer a --setopt=cachedir= option"
fi

RWP=$(sed -n 's/^ReadWritePaths=//p' "$UNIT" | tr ' ' '\n' | sed 's/^-//' | grep '^/' || true)
covered=""
for p in $RWP; do
  case "$CACHEDIR/" in "$p"/*) covered="$p" ;; esac
done
if [ -n "$CACHEDIR" ] && [ -n "$covered" ]; then
  ok "$CACHEDIR is inside ReadWritePaths entry $covered"
else
  bad "${CACHEDIR:-the cache dir} is not inside any ReadWritePaths entry — dnf would hit Errno 30 again"
fi

case "$CACHEDIR" in
  /var/cache/*) bad "the query cache points back into /var/cache, which is read-only for the agent" ;;
  *) ok "the query cache is not under the read-only /var/cache" ;;
esac

echo
echo "§2 every dnf query in the agent passes the writable cache"

OUT=$(scan "$AGENT_SRC")
HITS=$(grep -c '^HIT ' <<<"$OUT" || true)
MISSES=$(grep '^MISS ' <<<"$OUT" | sed 's/^MISS //' | tr '\n' ' ')
if [ "$HITS" -ge 5 ]; then
  ok "$HITS dnf query sites found"
else
  bad "only $HITS dnf query sites found — the scan is broken, not the code (a green here would mean nothing)"
fi
if [ -z "$MISSES" ]; then
  ok "all $HITS query sites pass DNF_QUERY_CACHEDIR"
else
  bad "these dnf queries use the read-only system cache and fail inside the sandbox: $MISSES"
fi

for fn in php_streams available; do
  body=$(code_of "$PKG" | awk "/^pub async fn $fn\\(/,/^}/")
  if grep -q 'DNF_QUERY_CACHEDIR' <<<"$body"; then
    ok "pkg::$fn passes the writable cache"
  else
    bad "pkg::$fn no longer passes DNF_QUERY_CACHEDIR"
  fi
done

echo
echo "§3 controls: the scan fires on the real defect and stays quiet on the fix"

FIX=$(mktemp -d)
trap 'rm -rf "$FIX"' EXIT
cat > "$FIX/defect.rs" <<'RS'
async fn list_extensions() {
    let out = safe_command("dnf")
        .args(["-q", "list", "--available", "php-*"])
        .output()
        .await;
}
RS
if grep -q '^MISS ' <<<"$(scan "$FIX")"; then
  ok "a query without the option is reported (negative control fired)"
else
  bad "the scan did not report a planted query without the option — it cannot fail"
fi

cat > "$FIX/defect.rs" <<'RS'
async fn list_extensions() {
    let out = safe_command("dnf")
        .args([crate::services::pkg::DNF_QUERY_CACHEDIR, "-q", "list", "--available", "php-*"])
        .output()
        .await;
}
RS
FIXED=$(scan "$FIX")
if grep -q '^HIT ' <<<"$FIXED" && ! grep -q '^MISS ' <<<"$FIXED"; then
  ok "the same query with the option passes (positive control)"
else
  bad "the scan rejects a correct query — it would fail the fix itself"
fi

cat > "$FIX/defect.rs" <<'RS'
// dnf answers `"repoquery"` and `"--available"` here without any cache option.
async fn noop() {}
RS
if grep -q '^HIT ' <<<"$(scan "$FIX")"; then
  bad "a comment quoting a query verb became a subject — prose can satisfy or fail an arm"
else
  ok "comments are not subjects (the comment stripper works)"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
