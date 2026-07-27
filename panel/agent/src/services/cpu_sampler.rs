//! Whole-system CPU usage, sampled on a fixed cadence.
//!
//! CPU percentage is a *delta over a window*, not a readable instant value. The
//! obvious implementation — refresh inside the request handler and return
//! `global_cpu_usage()` — makes the window "time since whichever unrelated
//! caller last hit this endpoint". The agent has five such callers (the metrics
//! collector every 30 s, the dashboard WebSocket loop every ~5 s, the dashboard
//! REST tick, the backup scheduler, the Docker-apps page), so every caller
//! silently corrupted every other caller's window.
//!
//! Two ways that produced a wrong number:
//!
//! * **Short windows are dominated by the agent's own work.** `/system/info`
//!   used to walk all of `/proc` on every call (~70 ms of CPU with 583
//!   processes). When two callers landed a few hundred ms apart, that walk was
//!   most of the measured window. The self-inflicted floor is
//!   `walk_ms / (gap_ms * cores) * 100` — ~2 % on a 12-core box at a 250 ms gap,
//!   but **~28 % on a single-core box, and higher once the WebSocket loop's
//!   concurrent `/system/processes` call (two more full walks) lands in the same
//!   window.** That is how an idle box reported ~99 %.
//! * **Windows under 200 ms silently returned a stale value.** sysinfo skips the
//!   refresh entirely below `MINIMUM_CPU_UPDATE_INTERVAL`, so the handler
//!   returned whatever the previous window had computed, with no indication.
//!
//! So the sampler owns the window instead. One task, its own `System`, a fixed
//! interval, publishing to an atomic that handlers read for free. The number no
//! longer depends on who asked or how often.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{CpuRefreshKind, RefreshKind, System};

/// Interval between samples, and therefore the averaging window of every value
/// this module publishes.
///
/// Must stay comfortably above sysinfo's `MINIMUM_CPU_UPDATE_INTERVAL` (200 ms
/// on Linux): at or below it, refreshes are *skipped* rather than rejected, and
/// the published value would silently stop advancing.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// Delay used to prime the first real sample at startup, so the agent publishes
/// a true (if short-windowed) reading a moment after boot instead of a zero.
const PRIME_DELAY: Duration = Duration::from_millis(300);

/// Handle to the latest whole-system CPU usage, 0-100.
///
/// Cheap to clone and free to read — the value is an `f32` stored as its bit
/// pattern, so readers never block the sampler or each other.
#[derive(Clone)]
pub struct CpuSampler {
    /// `f32::to_bits` of the most recent sample.
    value: Arc<AtomicU32>,
}

impl Default for CpuSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuSampler {
    pub fn new() -> Self {
        Self {
            value: Arc::new(AtomicU32::new(0f32.to_bits())),
        }
    }

    /// Latest sample, averaged over the preceding [`SAMPLE_INTERVAL`].
    ///
    /// Returns 0.0 until the first sample lands (~300 ms after start).
    pub fn usage(&self) -> f32 {
        f32::from_bits(self.value.load(Ordering::Relaxed))
    }

    fn publish(&self, usage: f32) {
        self.value.store(usage.to_bits(), Ordering::Relaxed);
    }

    /// Spawn the sampling loop. Call once, at startup.
    pub fn spawn(&self) {
        let this = self.clone();
        tokio::spawn(async move {
            // A CPU-usage-only `System`: no process list, no memory, no disks.
            // Refreshing it reads a single line of /proc/stat, so the sampler
            // does not meaningfully appear in its own measurement.
            let mut sys = System::new_with_specifics(
                RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_cpu_usage()),
            );

            // The constructor's refresh is the baseline; it has no predecessor
            // to diff against, so its value is not a measurement of anything.
            // Prime a real one quickly, then settle into the fixed cadence.
            tokio::time::sleep(PRIME_DELAY).await;
            sys.refresh_cpu_usage();
            this.publish(sys.global_cpu_usage());

            let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await; // interval() fires immediately; that tick is now.

            loop {
                ticker.tick().await;
                sys.refresh_cpu_usage();
                this.publish(sys.global_cpu_usage());
            }
        });
    }
}

/// Number of running processes, without walking every `/proc` entry's stats.
///
/// `/system/info` needs only the count, but obtaining it from sysinfo means
/// `refresh_processes(All, true)` — a full walk that reads and parses a stat
/// file per process. That walk was both the endpoint's dominant cost (~70 ms of
/// CPU per call) and the contaminant in the CPU window above, so the count is
/// taken by listing the directory instead.
pub fn process_count() -> usize {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.as_bytes().iter().all(|b| b.is_ascii_digit()))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the module: the window is a constant, and it is above
    /// the threshold below which sysinfo silently skips refreshing.
    #[test]
    fn sample_interval_exceeds_sysinfo_minimum() {
        assert!(
            SAMPLE_INTERVAL > sysinfo::MINIMUM_CPU_UPDATE_INTERVAL,
            "SAMPLE_INTERVAL ({SAMPLE_INTERVAL:?}) must exceed sysinfo's \
             MINIMUM_CPU_UPDATE_INTERVAL ({:?}), or refreshes are skipped and \
             the published value goes stale without any error",
            sysinfo::MINIMUM_CPU_UPDATE_INTERVAL
        );
        assert!(
            PRIME_DELAY > sysinfo::MINIMUM_CPU_UPDATE_INTERVAL,
            "PRIME_DELAY must also exceed the minimum, or the first published \
             sample is the skipped-refresh baseline rather than a measurement"
        );
    }

    #[test]
    fn usage_round_trips_through_the_atomic() {
        let s = CpuSampler::new();
        assert_eq!(s.usage(), 0.0, "a fresh sampler reports 0, not garbage");
        s.publish(37.5);
        assert_eq!(s.usage(), 37.5);
        // Clones share the cell, so handlers see the sampler's writes.
        let c = s.clone();
        s.publish(4.25);
        assert_eq!(c.usage(), 4.25);
    }

    #[test]
    fn process_count_is_plausible_and_digit_filtered() {
        let n = process_count();
        // /proc always holds at least this test's own process.
        assert!(n > 0, "process_count returned 0 on a live system");
        // Non-numeric /proc entries (self, stat, meminfo, …) must not be counted.
        let total = std::fs::read_dir("/proc").unwrap().count();
        assert!(
            n < total,
            "process_count ({n}) should exclude the non-pid entries in /proc ({total})"
        );
    }
}
