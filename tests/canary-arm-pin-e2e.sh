#!/usr/bin/env bash
# Regression pins for s380 — the intrusion tripwire that was never armed.
#
#   C1  `security_canary_enabled` is seeded 'true'
#       (migrations/20260324000000_security_enhancements.sql), Settings rendered a
#       "Canary File Monitoring" switch ON unless the stored value was literally
#       "false", and docs/guides/security-hardening.md promised checks of
#       `/etc/`, `/root/`, `/home/` and `/var/www/` every 2 minutes. The agent
#       endpoint that CREATES those files — POST /security/canary/setup — had
#       ZERO callers anywhere in the panel, the CLI or any installer. The only
#       panel-side mention of it was the author's own comment recording that it
#       has no caller. So on every install the tripwire watched zero files, and
#       an intrusion detector whose only observable behaviour is silence looks
#       exactly like one that is working.
#
#   C2  Three of the four advertised paths could never have been written by the
#       agent in the first place. The unit runs `ProtectSystem=strict` with an
#       explicit `ReadWritePaths=`, and bare `/etc`, `/root` and `/home` are not
#       in it — measured under the shipped unit with systemd-run, not inferred:
#       all three refuse a touch and only `/var/www` accepts one. `canary_setup`
#       wrapped each write in `if …is_ok()` with no `else` and returned only what
#       succeeded, so "armed 4 of 4" and "armed 1 of 4 in silence" were the same
#       response.
#
#   C3  `/root/` and `/home/` cannot be WATCHED either — both daemons run under
#       `ProtectHome=yes` — so planting them, by any means, changes nothing.
#
# The shape of the fix is the point, twice over:
#
#   * §1 is a DERIVED check over a CLASS, not a check for this endpoint. It
#     parses the agent's security router for every registered /security/* path
#     and asserts each one has a panel-side caller or an explicit allow-list
#     entry naming why not. The NEXT severed agent endpoint fails the same arm.
#   * §2 pins the plant set against the SANDBOX rather than against a list, so a
#     path added to either crate that the agent cannot write, or that the API
#     cannot read, fails without anyone remembering this ever happened.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

AGENT_SEC=panel/agent/src/routes/security.rs
AGENT_UNIT=panel/agent/dockpanel-agent.service
HEALER=panel/backend/src/services/auto_healer.rs
BACK_SEC=panel/backend/src/routes/security.rs
ROUTER=panel/backend/src/routes/mod.rs
SETTINGS=panel/frontend/src/pages/Settings.tsx
GUIDE=docs/guides/security-hardening.md

for f in "$AGENT_SEC" "$AGENT_UNIT" "$HEALER" "$BACK_SEC" "$ROUTER" "$SETTINGS" "$GUIDE"; do
  [ -f "$f" ] || { echo "missing source file: $f"; exit 1; }
done

echo
echo "── 1. every agent /security/* route has a panel-side caller ──"

# An agent endpoint nobody calls is the defect class this suite exists for. The
# allow-list carries a REASON, so a route can only leave the arm deliberately.
#
# `/security/db-backup` is superseded, not severed: auto_healer.rs runs the panel
# database dump itself with a direct `pg_dump`, under its own comment saying it
# "doesn't need agent client". That is a decision, and this is where it is recorded.
REACH=$(python3 - "$AGENT_SEC" <<'PY'
import os, re, sys

ALLOW = {
    "/security/db-backup":
        "superseded: auto_healer.rs dumps the panel DB with a direct pg_dump, "
        "documented in its own comment as not needing the agent client",
}

agent = open(sys.argv[1], encoding="utf-8").read()
routes = sorted(set(re.findall(r'\.route\(\s*"(/security/[^"]+)"', agent)))

# Assert the enumeration BEFORE trusting it. A parse that yields nothing reads
# exactly like "every route has a caller".
if len(routes) < 15:
    print(f"ENUM-FAIL agent security router yielded {len(routes)} routes")
    raise SystemExit

hay = []
for root in ("panel/backend/src", "panel/cli/src"):
    for d, _, fs in os.walk(root):
        for f in fs:
            if f.endswith(".rs"):
                hay.append(open(os.path.join(d, f), encoding="utf-8", errors="replace").read())
hay = "\n".join(hay)
if len(hay) < 500_000:
    print(f"ENUM-FAIL panel-side haystack is {len(hay)} bytes — parse is partial")
    raise SystemExit

# POSITIVE CONTROL: a route we know is called must be found by this same matcher,
# otherwise "nothing is severed" is indistinguishable from a broken matcher.
def called(route):
    stem = route.split("{")[0].rstrip("/") if "{" in route else route
    return stem in hay

if not called("/security/overview"):
    print("ENUM-FAIL positive control /security/overview not found — matcher is broken")
    raise SystemExit

severed = [r for r in routes if not called(r) and r not in ALLOW]
stale = [r for r in ALLOW if r not in routes]

print(f"ENUM-OK {len(routes)} agent security routes / {len(hay)} bytes panel-side")
for r in severed:
    print(f"SEVERED {r} — registered on the agent, called by nothing in the panel or CLI")
for r in stale:
    print(f"STALE-ALLOW {r} — allow-listed but no longer registered; the entry is rotting")
PY
)

case "$REACH" in
  *ENUM-FAIL*) bad "agent-route reachability enumeration failed: $(printf '%s' "$REACH" | grep 'ENUM-FAIL')" ;;
  *)
    ok "reachability enumeration is non-empty and its positive control fired — $(printf '%s' "$REACH" | grep 'ENUM-OK')"
    SEVERED_N=$(printf '%s\n' "$REACH" | grep -c 'SEVERED ')
    if [ "$SEVERED_N" -eq 0 ]; then
      ok "every agent /security/* route has a panel-side caller or a reasoned allow-list entry"
    else
      bad "$SEVERED_N agent /security/* route(s) are registered and called by nothing:"
      printf '%s\n' "$REACH" | grep 'SEVERED ' | sed 's/^/      /'
    fi
    STALE_N=$(printf '%s\n' "$REACH" | grep -c 'STALE-ALLOW ')
    if [ "$STALE_N" -eq 0 ]; then
      ok "the allow-list names only routes that still exist"
    else
      bad "$STALE_N allow-list entr(y|ies) name a route that no longer exists:"
      printf '%s\n' "$REACH" | grep 'STALE-ALLOW ' | sed 's/^/      /'
    fi
    ;;
esac

echo
echo "── 2. the canary plant set is bounded by the sandbox, not by memory ──"

SANDBOX=$(python3 - "$AGENT_SEC" "$HEALER" "$AGENT_UNIT" <<'PY'
import re, sys
agent, healer, unit = (open(p, encoding="utf-8").read() for p in sys.argv[1:4])

def const_paths(src, name):
    # Both shapes occur: a multi-line array of (path, description) tuples and a
    # single-line array of bare prefixes. A pattern that assumes a newline before
    # the closing bracket silently returns None for the second — which reads as
    # "the constant is missing" rather than "the parser is wrong".
    m = re.search(name + r'\s*:\s*\[\(?&str.*?\]\s*=\s*\[(.*?)\];', src, re.S)
    if not m:
        return None
    return re.findall(r'"(/[^"]+)"', m.group(1))

plant_agent = const_paths(agent, "CANARY_PLANT_PATHS")
plant_back  = const_paths(healer, "CANARY_PLANTABLE")
watch       = const_paths(healer, "CANARY_WATCH_PATHS")
masked      = const_paths(healer, "SANDBOX_MASKED_PREFIXES")

for label, v in (("CANARY_PLANT_PATHS", plant_agent), ("CANARY_PLANTABLE", plant_back),
                 ("CANARY_WATCH_PATHS", watch), ("SANDBOX_MASKED_PREFIXES", masked)):
    if not v:
        print(f"ENUM-FAIL could not parse {label}")
        raise SystemExit

# The agent's plant paths are file paths; the const holds (path, description)
# pairs, so keep only the ones that look like a canary file.
plant_agent = [p for p in plant_agent if p.endswith(".dockpanel-canary")]
plant_back  = [p for p in plant_back if p.endswith(".dockpanel-canary")]

rw = re.search(r'^ReadWritePaths=(.*)$', unit, re.M)
if not rw:
    print("ENUM-FAIL agent unit has no ReadWritePaths= line")
    raise SystemExit
# A leading '-' marks "only if it exists"; it is still a writable prefix.
writable = [t.lstrip('-') for t in rw.group(1).split() if t.startswith(('/', '-/'))]
if len(writable) < 5:
    print(f"ENUM-FAIL ReadWritePaths parsed to {len(writable)} entries")
    raise SystemExit

print(f"ENUM-OK plant={len(plant_agent)} watch={len(watch)} writable={len(writable)}")

if plant_agent != plant_back:
    print(f"DRIFT agent CANARY_PLANT_PATHS {plant_agent} != backend CANARY_PLANTABLE {plant_back}")

for p in plant_agent:
    parent = p.rsplit("/", 1)[0]
    if not any(parent == w or parent.startswith(w.rstrip("/") + "/") for w in writable):
        print(f"UNWRITABLE {p} — {parent} is not inside the agent unit's ReadWritePaths=, "
              f"so this write fails EROFS and is silently skipped")
    if any(p.startswith(m) for m in masked):
        print(f"MASKED {p} — under {[m for m in masked if p.startswith(m)][0]}, which "
              f"ProtectHome= hides from the process that reads it; planting it changes nothing")
    if p not in watch:
        print(f"UNWATCHED {p} — planted but not in CANARY_WATCH_PATHS, so nothing reads its atime")
PY
)

case "$SANDBOX" in
  *ENUM-FAIL*) bad "canary constant parse failed: $(printf '%s' "$SANDBOX" | grep 'ENUM-FAIL')" ;;
  *)
    ok "canary constants parsed on both sides — $(printf '%s' "$SANDBOX" | grep 'ENUM-OK')"
    for tag in DRIFT UNWRITABLE MASKED UNWATCHED; do
      N=$(printf '%s\n' "$SANDBOX" | grep -c "^$tag ")
      case "$tag" in
        DRIFT)      MSG="the agent's plant set and the backend's mirror are identical" ;;
        UNWRITABLE) MSG="every planted canary is inside the agent unit's ReadWritePaths=" ;;
        MASKED)     MSG="no planted canary sits under a ProtectHome=-masked prefix" ;;
        UNWATCHED)  MSG="every planted canary is also in the watch set" ;;
      esac
      if [ "$N" -eq 0 ]; then ok "$MSG"; else
        bad "$MSG — violated $N time(s):"
        printf '%s\n' "$SANDBOX" | grep "^$tag " | sed 's/^/      /'
      fi
    done
    ;;
esac

echo
echo "── 3. the endpoint reports refusals instead of swallowing them ──"

# The defect was `if std::fs::write(path, &content).is_ok() { … }` with no else:
# a refused write left no trace in the response. Key the arm on the OPERATION,
# not on a sentence describing it — the comments in this file discuss the old
# shape, so a bare token search would match the prose that narrates the fix.
SWALLOW=$(grep -c 'fs::write(path, &content)\.is_ok()' "$AGENT_SEC")
if [ "$SWALLOW" -eq 0 ]; then
  ok "canary_setup no longer discards a failed write with a bare is_ok() guard"
else
  bad "canary_setup still guards its write with .is_ok() at $SWALLOW site(s) — a refusal is invisible again"
fi

REFUSED=$(grep -c '"refused": refused' "$AGENT_SEC")
if [ "$REFUSED" -ge 1 ]; then
  ok "canary_setup returns a refused list, so a partial arm is distinguishable from a full one"
else
  bad "canary_setup does not return a refused list — 'armed 1 of 4' reads as success again"
fi

echo
echo "── 4. the panel reports the TRIPWIRE, not the setting ──"

STATUS_ROUTE=$(grep -c 'security::canary_status' "$ROUTER")
ARM_ROUTE=$(grep -c 'security::canary_arm' "$ROUTER")
if [ "$STATUS_ROUTE" -ge 1 ] && [ "$ARM_ROUTE" -ge 1 ]; then
  ok "canary-status and canary/arm are both registered in the router"
else
  bad "canary routes missing from the router (status=$STATUS_ROUTE arm=$ARM_ROUTE)"
fi

# POSITIVE CONTROL for the two absence-shaped reads below: the file really does
# contain the canary row, so a zero here would mean a broken path, not a clean UI.
ROW=$(grep -c 'Canary File Monitoring' "$SETTINGS")
if [ "$ROW" -ge 1 ]; then
  ok "control: the Settings canary row is present in $SETTINGS ($ROW occurrence(s))"
else
  bad "control failed: no canary row found in $SETTINGS — the arms below cannot be trusted"
fi

# Key on the QUOTED path, not the bare substring. `grep -c 'security/canary-status'`
# also matches `"/security/canary-status-DISABLED"`, so the arm survived a mutation
# that pointed the screen at a route the router does not serve (#523).
FETCH=$(grep -c '"/security/canary-status"' "$SETTINGS")
if [ "$FETCH" -ge 1 ]; then
  ok "Settings asks the backend what is actually armed"
else
  bad "Settings never calls /security/canary-status — the switch is reporting the setting again"
fi

NOTARMED=$(grep -c 'NOT ARMED' "$SETTINGS")
if [ "$NOTARMED" -ge 1 ]; then
  ok "Settings renders an explicit NOT ARMED state for a tripwire watching nothing"
else
  bad "Settings has no NOT ARMED state — zero watched paths paints as protected again"
fi

echo
echo "── 4b. arming does not raise its own intrusion alert ──"

# Planting is a write, so the atime moves and the sweeper reads it as access.
# Without clearing the stored baseline, arming a host that already had canaries
# fires one CANARY TRIGGERED per existing file — each a critical audit entry, an
# admin notification, and a suspicious event feeding the 5-in-10-minutes
# auto-lockdown. Four pre-existing canaries is four events. (Fixed v2.132.1.)
CLEARS=$(grep -c 'DELETE FROM settings WHERE key = \$1' "$BACK_SEC")
if [ "$CLEARS" -ge 1 ]; then
  ok "canary_arm clears the stored access-time baseline for the paths it plants"
else
  bad "canary_arm leaves the old baseline in place — arming an armed host alerts on every existing canary and can trip auto-lockdown"
fi

# The key is built in two places: the sweeper WRITES it, the arm handler DELETES
# it. Spelled differently they never meet, and the failure is silent in the
# alerting direction.
W=$(grep -c 'canary_atime_{}' "$HEALER")
D=$(grep -c 'canary_atime_{}' "$BACK_SEC")
if [ "$W" -ge 1 ] && [ "$D" -ge 1 ]; then
  ok "the sweeper's baseline key and the arm handler's delete share one format (writer=$W deleter=$D)"
else
  bad "baseline key format drifted between writer and deleter (writer=$W deleter=$D) — the DELETE cannot match the row"
fi

echo
echo "── 5. the guide states what the tripwire cannot do ──"

CANARY_SECTION=$(python3 - "$GUIDE" <<'PY'
import sys
src = open(sys.argv[1], encoding="utf-8").read()
if "### Canary Files" not in src:
    print("ENUM-FAIL no '### Canary Files' section in the guide")
    raise SystemExit
sec = src.split("### Canary Files", 1)[1]
# Bound the window on the next heading, never on a fixed line count: a fixed
# -A window bleeds into the neighbouring section, which here is the one whose
# disclaimer wording this arm searches for.
nxt = sec.find("\n### ")
sec = sec[:nxt] if nxt != -1 else sec
if len(sec) < 200:
    print(f"ENUM-FAIL canary section extracted to {len(sec)} chars")
    raise SystemExit
print(f"ENUM-OK canary section is {len(sec)} chars")
if "What this does not do, stated plainly" not in sec:
    print("NODISCLAIMER the section makes claims with no limits paragraph")
for p in ("/root/", "/home/"):
    if p in sec and "ProtectHome" not in sec:
        print(f"UNQUALIFIED names {p} without saying it cannot be watched")
if "Arm canary files" not in sec:
    print("NOARM the section never tells the reader the tripwire must be armed first")
PY
)

case "$CANARY_SECTION" in
  *ENUM-FAIL*) bad "guide section parse failed: $(printf '%s' "$CANARY_SECTION" | grep 'ENUM-FAIL')" ;;
  *)
    ok "guide canary section extracted — $(printf '%s' "$CANARY_SECTION" | grep 'ENUM-OK')"
    for tag in NODISCLAIMER UNQUALIFIED NOARM; do
      N=$(printf '%s\n' "$CANARY_SECTION" | grep -c "^$tag")
      case "$tag" in
        NODISCLAIMER) MSG="the guide carries a 'What this does not do, stated plainly' paragraph" ;;
        UNQUALIFIED)  MSG="the guide never names /root/ or /home/ without the ProtectHome caveat" ;;
        NOARM)        MSG="the guide tells the reader the tripwire must be armed" ;;
      esac
      if [ "$N" -eq 0 ]; then ok "$MSG"; else
        bad "$MSG — violated:"
        printf '%s\n' "$CANARY_SECTION" | grep "^$tag" | sed 's/^/      /'
      fi
    done
    ;;
esac

echo
echo "──────────────────────────────────────────"
printf 'PASS %d  FAIL %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
