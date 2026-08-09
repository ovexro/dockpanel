#!/usr/bin/env bash
# Regression pins for s269 — the CPU number that measured the caller, not the box.
#
# The dashboard gauge read 99% on a box `top` reported 94.6% idle, while the
# 24-hour chart built from the SAME endpoint read 9.7%. Nothing was wrong with
# the box; the number was an artefact of how it was asked for.
#
#   C1  CPU percentage is a delta over a window. `/system/info` refreshed CPU
#       inside the handler, so the window was "time since whichever unrelated
#       caller last hit this endpoint". Five callers share it (metrics collector
#       every 30 s, the dashboard WebSocket loop every ~5 s, the dashboard REST
#       tick, the backup scheduler, the Docker-apps page), so each corrupted the
#       others' windows. The value was never a property of the system.
#   C2  The handler walked every process in /proc on each call — measured at
#       ~70 ms of CPU with 583 processes — purely to obtain a COUNT. That walk
#       landed inside the next caller's window, so the endpoint was the largest
#       consumer in its own measurement. The self-inflicted floor is
#       walk_ms / (gap_ms * cores) * 100: ~2% on a 12-core box at a 250 ms gap,
#       ~28% on a single core, and higher still once the WebSocket loop's
#       concurrent /system/processes call (two more full walks) lands in the
#       same window.
#   C3  Below sysinfo's MINIMUM_CPU_UPDATE_INTERVAL (200 ms) a refresh is
#       SKIPPED, not rejected — so closely-spaced callers silently received a
#       value computed for an older window, with no error anywhere.
#   C4  phone_home built `System::new_all()` then `refresh_all()` back to back
#       and read global_cpu_usage() off it: a window consisting almost entirely
#       of its own /proc walk. On an idle 12-core box whose true usage was 4.5%,
#       consecutive runs returned 4.5%–22.1%. That value lands in
#       servers.cpu_usage, which the Servers page shows and the alert engine
#       compares against CPU thresholds.
#
# The fix is structural: one sampler owns the window. A dedicated task with its
# own CPU-only System refreshes on a fixed cadence and publishes to an atomic;
# handlers read it. These pins hold that shape, because the defect returns the
# moment any handler refreshes CPU for itself again.
#
# Pure source analysis: no box, no network, no build.

set -uo pipefail
cd "$(dirname "$0")/.."

PASS=0 FAIL=0
ok()  { PASS=$((PASS+1)); printf '  \033[0;32m✓\033[0m %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  \033[0;31m✗\033[0m %s\n' "$1"; }

# Strip comments and the test module, so a pin can never be satisfied (or
# tripped) by prose describing the very thing it forbids.
code()  { sed '/#\[cfg(test)\]/q' "$1" | grep -vE '^[[:space:]]*(///|//!|//|\*|/\*)'; }
has()   { [ -n "$(code "$1" | grep -F -- "$2")" ]; }
hasre() { [ -n "$(code "$1" | grep -E -- "$2")" ]; }

SAMPLER=panel/agent/src/services/cpu_sampler.rs
SYSROUTE=panel/agent/src/routes/system.rs
APPSTATE=panel/agent/src/routes/mod.rs
MAIN=panel/agent/src/main.rs
PHONE=panel/agent/src/services/phone_home.rs
COLLECTOR=panel/backend/src/services/metrics_collector.rs

for f in "$SAMPLER" "$SYSROUTE" "$APPSTATE" "$MAIN" "$PHONE" "$COLLECTOR"; do
  [ -f "$f" ] || { echo "missing source file: $f"; exit 1; }
done

echo "── 1. C1: no handler computes CPU from its own request timing ──"

# The heart of it. A refresh inside the handler re-creates the defect exactly,
# whatever else changes around it.
if has "$SYSROUTE" "refresh_cpu"; then
  bad "routes/system.rs refreshes CPU in the handler again — the window is back to 'time since some other caller'"
else
  ok "routes/system.rs does not refresh CPU per request"
fi

if hasre "$SYSROUTE" "state\.cpu\.usage\(\)"; then
  ok "routes/system.rs reads the sampled value"
else
  bad "routes/system.rs no longer reads the sampler — where is cpu_usage coming from?"
fi

# Any OTHER agent route computing CPU for itself is the same bug wearing a
# different URL, so pin the whole routes tree rather than the one file.
STRAY=$(grep -rln "global_cpu_usage\|refresh_cpu" panel/agent/src/routes/ 2>/dev/null | grep -v "^$SYSROUTE$" || true)
if [ -z "$STRAY" ]; then
  ok "no other agent route computes CPU usage on its own"
else
  bad "these routes compute CPU themselves, re-opening the per-caller window: $STRAY"
fi

echo
echo "── 2. C2: the endpoint is not the biggest consumer in its own measurement ──"

# Scope this to the system_info HANDLER. The sibling /system/processes endpoint
# must walk /proc — that is the data it returns — and it does so on its own
# System instance. The defect is the *info* handler paying that cost to obtain a
# count, inside the window the next caller measures.
info_body() { code "$SYSROUTE" | sed -n '/async fn system_info/,/^}/p'; }
if [ -n "$(info_body | grep -F 'refresh_processes')" ]; then
  bad "system_info walks every process again (~70 ms of CPU per call) — that walk lands inside the next caller's window"
else
  ok "system_info does not walk every process"
fi
# Guard the guard: if the handler is renamed the extraction silently matches
# nothing and the pin above would pass vacuously.
if [ -n "$(info_body)" ]; then
  ok "the system_info handler was located for scoped inspection"
else
  bad "could not locate the system_info handler — the scoped pins above are vacuous"
fi

if hasre "$SYSROUTE" "process_count\(\)"; then
  ok "the process count comes from a cheap /proc listing"
else
  bad "process_count is sourced some other way — check it does not refresh per-process stats"
fi

# The sampler must not reintroduce the cost it exists to remove: a CPU-only
# System reads one line of /proc/stat and does not appear in its own numbers.
if hasre "$SAMPLER" "RefreshKind::nothing\(\)"; then
  ok "the sampler's System is scoped to CPU usage only"
else
  bad "the sampler refreshes more than CPU — it will show up in its own measurement"
fi
if has "$SAMPLER" "refresh_processes"; then
  bad "the sampler walks processes — it now contaminates the window it owns"
else
  ok "the sampler does not walk processes"
fi

echo
echo "── 3. C3: the window is fixed, and above the interval sysinfo silently skips ──"

if has "$SAMPLER" "SAMPLE_INTERVAL"; then
  ok "the sampling window is a named constant, not an incidental gap"
else
  bad "no SAMPLE_INTERVAL — the window is implicit again"
fi

# Pin the INVARIANT (window > sysinfo's skip threshold), not the literal value,
# so tuning the cadence is allowed and breaking the guarantee is not. sysinfo's
# MINIMUM_CPU_UPDATE_INTERVAL is 200 ms on Linux.
check_duration_above_200ms() {
  local name="$1" line ms
  line=$(code "$SAMPLER" | grep -F "$name" | grep -F "Duration::from_" | head -1)
  if [ -z "$line" ]; then bad "$name is not a Duration literal — cannot verify it clears sysinfo's 200 ms threshold"; return; fi
  if printf '%s' "$line" | grep -cF "from_secs" >/dev/null; then
    ms=$(printf '%s' "$line" | sed -n 's/.*from_secs(\([0-9]*\)).*/\1/p'); ms=$((ms * 1000))
  else
    ms=$(printf '%s' "$line" | sed -n 's/.*from_millis(\([0-9]*\)).*/\1/p')
  fi
  if [ -n "$ms" ] && [ "$ms" -gt 200 ]; then
    ok "$name is ${ms}ms — above sysinfo's 200ms skip threshold"
  else
    bad "$name is '${ms:-unparsed}'ms — at or below 200ms sysinfo SKIPS the refresh and the published value silently goes stale"
  fi
}
check_duration_above_200ms "SAMPLE_INTERVAL"
check_duration_above_200ms "PRIME_DELAY"

if hasre "$MAIN" "cpu_sampler::CpuSampler::new\(\)" && has "$MAIN" ".spawn()"; then
  ok "main.rs creates and spawns the sampler"
else
  bad "the sampler is never spawned — every reader gets 0.0 forever"
fi
if hasre "$APPSTATE" "pub cpu:"; then
  ok "AppState carries the sampler so handlers can read it without refreshing"
else
  bad "AppState has no sampler handle"
fi

echo
echo "── 4. C4: the fleet check-in reports a measurement, not its own /proc walk ──"

if has "$PHONE" "global_cpu_usage"; then
  bad "phone_home reads global_cpu_usage off a System it just built — that number is mostly its own /proc walk, and it drives the Servers page and CPU alerts"
else
  ok "phone_home does not compute CPU from a freshly-built System"
fi
if hasre "$PHONE" "cpu\.usage\(\)"; then
  ok "phone_home reports the sampled value"
else
  bad "phone_home no longer reports the sampled value"
fi
# new_all()/refresh_all() is the specific pairing that produced 4.5%-22.1% on an
# idle box: the constructor refreshes, the walk runs, refresh_all() diffs against it.
if hasre "$PHONE" "System::new_all\(\)"; then
  bad "phone_home builds System::new_all() again — a full /proc walk whose cost becomes the CPU reading"
else
  ok "phone_home builds a narrowed System, not new_all()"
fi

echo
echo "── 5. the gauge and the chart still share one source ──"

# They diverged visibly (99% vs 9.7%) and that divergence was the tell. Both
# must keep reading the same endpoint, or a future defect can hide in the gap.
if has "$COLLECTOR" "/system/info" && has "$COLLECTOR" "cpu_usage"; then
  ok "the history collector still reads cpu_usage from /system/info"
else
  bad "the 24h chart no longer shares a source with the gauge — they can now disagree undetectably"
fi

echo
printf 'passed: %d   failed: %d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
