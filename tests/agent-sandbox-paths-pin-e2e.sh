#!/usr/bin/env bash
#
# Regression pin for the agent's systemd sandbox — the enumeration miss that has
# now shipped three times.
#
# The agent runs under `ProtectSystem=strict`, so every path it writes must be
# listed in `ReadWritePaths=`. When one is missing the write fails at the KERNEL
# with `Read-only file system (os error 30)` — no permission error, no clue in
# the feature's own code, and until v2.48.2 the panel rendered it as "Agent
# offline". The feature simply does not work, on every install, forever.
#
#   /etc/apt          listed unconditionally instead of with systemd's `-`
#                     prefix, so the unit would not START on any RPM box.
#   /var/spool/cron   missing from the unit while setup.sh pre-created it, so
#                     every cron written through the panel failed (s268).
#   /etc/ssh          missing entirely, so disable-root-login, disable-password
#                     and change-port ALL failed — found by a reporter on #92,
#                     reproduced on a fresh Ubuntu 24.04 box (s288).
#
# Three instances of one class, each found by a user rather than by us, so this
# stops being a comment and becomes a check.
#
# HOW THE SUBJECTS ARE FOUND. Not from a list — a list grows by one line each
# time someone is bitten, and the next path added to the codebase is not on it
# (lesson #100). Instead: every function in the agent that performs a Rust
# filesystem WRITE is located, and every absolute /etc, /var or /opt literal
# ANYWHERE IN THAT FUNCTION is treated as a candidate. Scoping to the function
# rather than the write line matters — `modify_sshd_config` writes through a
# `format!`ed temp path, so a line-scoped grep sees no literal at all and would
# have missed the very bug this pin exists for.
#
# EXEMPTIONS ARE JUSTIFIED, NOT ASSUMED. A candidate the agent does not actually
# write is exempted below WITH ITS REASON. That list is not the subject list: new
# paths are still discovered, and an unclassified one FAILS. Fail-closed is the
# point — a path nobody has thought about is exactly the bug.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

UNIT=panel/agent/dockpanel-agent.service
AGENT_SRC=panel/agent/src
[ -f "$UNIT" ] || { echo "missing $UNIT"; exit 1; }

# Paths that reach the discovery but are not agent writes. Each needs a reason.
is_exempt() {
  case "$1" in
    # A path the security scanner LOOKS FOR; it is never created by us.
    /etc/.dockpanel-canary) return 0 ;;
    # Read-only lookups.
    /var/lib/unbound/root.key|/usr/share/*) return 0 ;;
    # READ (routes/security.rs read_to_string) and a member of command_filter's
    # and the scanner's DENYLISTS. Exempt with prejudice: adding it to
    # ReadWritePaths would grant write access to the account database in order to
    # satisfy a check about a path the agent exists to keep commands away from.
    /etc/passwd|/etc/shadow|/etc/sudoers) return 0 ;;
    # `/var/run` bare: the agent writes /var/run/dockpanel, which IS listed. The
    # bare prefix appears in log/format strings.
    /var/run) return 0 ;;
    *) return 1 ;;
  esac
}

# Strip the test module to end-of-file before anything else. files.rs's
# symlink-escape tests reference /etc/passwd and /etc/cron.d/... as ATTACK
# targets they assert are refused; counting those as things the agent writes
# would demand the sandbox be widened to exactly the paths it must never allow.
# (Same trap as #94c, three times now.)
#
# Match ANY cfg predicate containing `test`, not the literal `#[cfg(test)]`:
# files.rs guards its module with `#[cfg(all(test, unix))]`, and a pattern that
# knows only one spelling silently lets that whole module back in — which is how
# the three attack-target paths above reached this pin's output on its first run.
code_of() { awk '/^#\[cfg\(.*\<test\>.*\)\]/ { exit } { print }' "$1"; }

echo "§1 the unit still declares a sandbox at all"

if grep -q '^ProtectSystem=strict' "$UNIT"; then
  ok "ProtectSystem=strict"
else
  bad "ProtectSystem is no longer strict — this pin, and the sandbox, mean nothing"
fi

RWP=$(sed -n 's/^ReadWritePaths=//p' "$UNIT" | tr ' ' '\n' | sed 's/^-//' | grep '^/' || true)
RWP_N=$(printf '%s\n' "$RWP" | grep -c . || true)
if [ "$RWP_N" -ge 20 ]; then
  ok "ReadWritePaths declares $RWP_N paths"
else
  bad "only $RWP_N ReadWritePaths entries parsed — the pattern is broken, not the unit (#100)"
fi

echo
echo "§2 optional paths carry systemd's '-' prefix (an absent one must not brick the unit)"

# /etc/nginx, /etc/dockpanel and the /var/{run,lib,log,www} family are created by
# the installer on every distro. Everything distro- or service-conditional needs
# the prefix, or a box without it fails namespace setup and the agent never starts.
MISSING_DASH=0
for p in /etc/letsencrypt /etc/apt /etc/fail2ban /etc/powerdns /etc/modsecurity \
         /etc/cloudflared /etc/postfix /etc/dovecot /var/spool/postfix /var/spool/cron \
         /var/vmail /var/cache/nginx /etc/ufw /var/lib/ufw /etc/php /etc/ssh; do
  if grep -q -- "-$p" "$UNIT"; then :; else
    bad "$p is listed without the '-' prefix — one absent directory stops the agent starting"
    MISSING_DASH=1
  fi
done
[ "$MISSING_DASH" -eq 0 ] && ok "all 16 conditional paths carry '-'"

echo
echo "§3 the three that already shipped broken stay listed"

for p in /etc/apt /var/spool/cron /etc/ssh; do
  if printf '%s\n' "$RWP" | grep -cx -- "$p" >/dev/null; then
    ok "$p"
  else
    bad "$p is gone from ReadWritePaths — this exact removal has already shipped as a bug once"
  fi
done

echo
echo "§4 every path the agent writes is covered (discovered, not listed)"

WRITE_RE='fs::(write|create_dir_all|copy|rename)|File::create'
CANDIDATES=$(
  for f in $(grep -rlE "$WRITE_RE" --include='*.rs' "$AGENT_SRC" 2>/dev/null); do
    code_of "$f" | awk -v re="$WRITE_RE" '
      /^[ \t]*(pub )?(async )?fn / { if (body ~ re) print paths; body=""; paths="" }
      { body = body $0 "\n"
        if (match($0, /"\/(etc|var|opt)\/[A-Za-z0-9_.\/-]+"/))
          paths = paths substr($0, RSTART+1, RLENGTH-2) "\n" }
      END { if (body ~ re) print paths }
    '
  done | grep '^/' | sort -u
)
CAND_N=$(printf '%s\n' "$CANDIDATES" | grep -c . || true)

if [ "$CAND_N" -ge 25 ]; then
  ok "$CAND_N write-path literals discovered across the agent"
else
  bad "only $CAND_N candidates discovered — the discovery is broken, and a broken pattern must not read as a clean sweep (#100)"
fi

UNCOVERED=0
while read -r p; do
  [ -n "$p" ] || continue
  is_exempt "$p" && continue
  covered=no
  for r in $RWP; do
    case "$p" in "$r"|"$r"/*) covered=yes; break ;; esac
  done
  if [ "$covered" = no ]; then
    bad "$p is written by the agent and is NOT under any ReadWritePaths entry — it will fail with 'Read-only file system' on every install"
    UNCOVERED=1
  fi
done <<< "$CANDIDATES"
[ "$UNCOVERED" -eq 0 ] && ok "every discovered write path is covered by the sandbox"

echo
echo "§5 the installers derive this list rather than restating it"

# Both mirrors used to be maintained by hand and both had drifted. They now parse
# the unit; if either stops, the next entry added here silently fails to have its
# directory pre-created.
for s in scripts/setup.sh scripts/update.sh; do
  if grep -q "s/\^ReadWritePaths=//p" "$s"; then
    ok "$(basename "$s") derives the path list from the unit"
  else
    bad "$(basename "$s") no longer parses ReadWritePaths from the unit — the mirrors will drift again"
  fi
done

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
