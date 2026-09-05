//! Per-site concurrency control.
//!
//! A second-pass fan-out audit on `wp_vulnerability.rs` (s468) found that
//! `apply_hardening` writes `wp-config.php` directly while `update_with_rollback`
//! independently snapshots/updates/rolls-back the SAME file tree, with no
//! coordination between them — a concurrent hardening call can be silently
//! discarded by a rollback whose snapshot predates it. Auditing every other
//! site-file-mutating entry point in the agent crate (`grep -rn
//! "Mutex|DashMap|RwLock" panel/agent/src/`) turned up zero locking keyed by
//! domain anywhere: backup restore, staging file sync/clone/delete, and git
//! deploy all have the identical gap. This module is the one shared fix.
//!
//! Every operation that mutates a site's on-disk files takes [`lock_site`] (or
//! [`lock_sites`] for a two-site operation like staging clone/sync) for the
//! full duration of the mutation. It is intentionally a single in-process
//! registry, not per-subsystem locks — the whole point is that two DIFFERENT
//! subsystems touching the same site now serialize against each other too.
//!
//! ⚠ Do not acquire a second lock from inside a function that already holds
//! one for the same key — `tokio::sync::Mutex` is not reentrant and a nested
//! same-key acquire deadlocks. `git_build::deploy_or_update` calls
//! `blue_green_update` internally; only the outer call takes the lock.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

type Registry = StdMutex<HashMap<String, Arc<AsyncMutex<()>>>>;

static SITE_LOCKS: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    SITE_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn entry(key: &str) -> Arc<AsyncMutex<()>> {
    // The std Mutex here only ever guards a HashMap insert/lookup — never an
    // `.await` — so it is held for nanoseconds and cannot itself deadlock or
    // block the async runtime.
    let mut map = registry().lock().unwrap_or_else(|p| p.into_inner());
    map.entry(key.to_string())
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

/// Acquire the sole in-process lock for `key`, held until the returned guard
/// is dropped. Callers should hold the guard for the FULL duration of the
/// site mutation, not just the write itself, and bind it to a named
/// variable (`let _guard = ...`) — a bare `let _ = ...` drops immediately.
///
/// `key` is namespaced by caller convention, not by this function: a plain
/// domain (`example.com`) for every WordPress/backup/staging site operation,
/// and `git-deploy:{name}` for git-deploy operations, which are keyed by
/// deployment name rather than domain and would otherwise risk colliding
/// with an unrelated domain-keyed lock if a deploy's name ever looked like
/// one.
pub async fn lock_site(key: &str) -> OwnedMutexGuard<()> {
    entry(key).lock_owned().await
}

/// Acquire locks for two sites (e.g. staging clone/sync's source+target) in a
/// fixed, key-sorted order — never call-argument order — so two concurrent
/// operations naming the same pair in opposite directions cannot deadlock by
/// each holding one lock and waiting on the other.
pub async fn lock_sites(a: &str, b: &str) -> (OwnedMutexGuard<()>, Option<OwnedMutexGuard<()>>) {
    if a == b {
        return (lock_site(a).await, None);
    }
    if a < b {
        let ga = lock_site(a).await;
        let gb = lock_site(b).await;
        (ga, Some(gb))
    } else {
        let gb = lock_site(b).await;
        let ga = lock_site(a).await;
        (ga, Some(gb))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn two_locks_on_the_same_key_serialize() {
        let order = Arc::new(StdMutex::new(Vec::new()));
        let o1 = order.clone();
        let o2 = order.clone();

        let first_started = Arc::new(tokio::sync::Notify::new());
        let fs1 = first_started.clone();

        let h1 = tokio::spawn(async move {
            let _guard = lock_site("same.example").await;
            o1.lock().unwrap().push("first-in");
            fs1.notify_one();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            o1.lock().unwrap().push("first-out");
        });

        first_started.notified().await;
        let h2 = tokio::spawn(async move {
            let _guard = lock_site("same.example").await;
            o2.lock().unwrap().push("second-in");
        });

        h1.await.unwrap();
        h2.await.unwrap();

        // "second-in" must never appear before "first-out" — proves the second
        // acquire genuinely blocked on the first, not just that both ran.
        let seq = order.lock().unwrap().clone();
        assert_eq!(seq, vec!["first-in", "first-out", "second-in"]);
    }

    #[tokio::test]
    async fn different_keys_do_not_block_each_other() {
        let _g1 = lock_site("a.example").await;
        // A different key must acquire immediately even while a.example is held.
        let fut = lock_site("b.example");
        let res = tokio::time::timeout(std::time::Duration::from_millis(200), fut).await;
        assert!(res.is_ok(), "lock on a different key blocked — keys are not independent");
    }

    #[tokio::test]
    async fn lock_sites_same_domain_returns_one_guard() {
        let (_g, second) = lock_sites("same.example", "same.example").await;
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn lock_sites_opposite_argument_order_does_not_deadlock() {
        // a,b and b,a must acquire in the same canonical order — if they didn't,
        // two concurrent calls naming the pair in opposite directions could each
        // hold one lock while waiting on the other, forever. Each side drops its
        // guards the instant it acquires them (`let _ =`, not `let _guard =`,
        // deliberately — a bound name would keep both locks held until this test
        // function returns and make every run look deadlocked regardless of
        // ordering).
        let fut1 = async { let _ = lock_sites("alpha.example", "beta.example").await; };
        let fut2 = async { let _ = lock_sites("beta.example", "alpha.example").await; };
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            async { tokio::join!(fut1, fut2); },
        ).await;
        assert!(res.is_ok(), "lock_sites deadlocked on opposite argument order");
    }
}
