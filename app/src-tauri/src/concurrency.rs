//! Process-global cap on how many agent CLI sessions (Claude/Codex/
//! Grok, one-shot or multi-stage) may run at once.
//!
//! Nothing else in the runner limits concurrency: every `submit_feedback`
//! with `autoEvolve` set, or every reviewer clicking Advance, spawns its own
//! OS thread which immediately launches an agent CLI plus whatever dev
//! tooling (`cargo`, `trunk`) that agent shells out to. With no cap, N
//! simultaneous submissions spawn N simultaneous agent + build-tool trees,
//! which can starve the host's CPU/memory or blow past API rate limits.
//!
//! [`AgentSlot::acquire`] blocks the calling thread until a slot is free,
//! then returns an RAII guard that releases the slot on drop — including on
//! early return or panic — so a permit can never leak.

use std::sync::{Condvar, Mutex, OnceLock};

const DEFAULT_MAX_CONCURRENT_JOBS: usize = 2;

struct Semaphore {
    state: Mutex<usize>,
    condvar: Condvar,
    cap: usize,
}

fn semaphore() -> &'static Semaphore {
    static SEM: OnceLock<Semaphore> = OnceLock::new();
    SEM.get_or_init(|| Semaphore {
        state: Mutex::new(0),
        condvar: Condvar::new(),
        cap: max_concurrent_jobs(),
    })
}

/// Read `EVOLVO_MAX_CONCURRENT_JOBS`, falling back to
/// [`DEFAULT_MAX_CONCURRENT_JOBS`] when unset or unparseable. Only accepts
/// values `>= 1` — a configured `0` (or negative, or non-numeric) would
/// either deadlock every job forever or disable the cap outright, neither
/// of which is a safe interpretation of "garbage input", so we fall back to
/// the default instead of honouring it.
fn max_concurrent_jobs() -> usize {
    std::env::var("EVOLVO_MAX_CONCURRENT_JOBS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n >= 1)
        .unwrap_or(DEFAULT_MAX_CONCURRENT_JOBS)
}

/// The configured concurrency cap, resolved once per process (subsequent
/// env var changes are not observed — same "restart to re-probe" convention
/// as `agent::is_installed`). Exposed for `app_health`.
pub fn cap() -> usize {
    semaphore().cap
}

/// Number of agent sessions currently holding a slot. Exposed for
/// `app_health` so the UI can show "N / cap active" without new UI work.
pub fn active_count() -> usize {
    *semaphore().state.lock().unwrap_or_else(|e| e.into_inner())
}

/// RAII permit for one agent session. Blocks in [`AgentSlot::acquire`] until
/// a slot is free; releases automatically on drop (covers early `return`,
/// `?`, and panics unwinding through the guard).
pub struct AgentSlot {
    _private: (),
}

impl AgentSlot {
    /// Block the calling thread until fewer than `cap` sessions are active,
    /// then reserve a slot and return the guard. Call `on_wait` at most once
    /// if this call actually has to wait (i.e. skip it on the common case
    /// where a slot is immediately free) — the runner uses it to append a
    /// "queued" job note so the UI shows why nothing is happening yet, and
    /// posting it unconditionally would spam a note on every job even when
    /// there was no queueing.
    pub fn acquire(on_wait: impl FnOnce(usize)) -> Self {
        let sem = semaphore();
        let mut count = sem.state.lock().unwrap_or_else(|e| e.into_inner());
        if *count >= sem.cap {
            on_wait(*count);
            while *count >= sem.cap {
                count = sem.condvar.wait(count).unwrap_or_else(|e| e.into_inner());
            }
        }
        *count += 1;
        Self { _private: () }
    }
}

impl Drop for AgentSlot {
    fn drop(&mut self) {
        let sem = semaphore();
        let mut count = sem.state.lock().unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
        // Only one waiter can make progress per release, but notify_all is
        // cheap here (cap is small) and avoids any risk of a missed wakeup
        // if multiple threads raced into `wait`.
        sem.condvar.notify_all();
    }
}

/// Process-global mutex ensuring at most one promotion (lineage-branch
/// merge into the source repo) runs at a time. Unlike `AgentSlot`, this is
/// NOT a queue — two simultaneous merges into the same source repo is
/// never safe (interleaved `git merge`/`git reset --hard` calls would
/// corrupt each other's view of HEAD), so a second caller must refuse
/// rather than block indefinitely behind the first. `try_acquire` returns
/// `None` immediately if a promotion is already running; the caller is
/// expected to append a job note and let the reviewer retry.
///
/// Implemented as a `Mutex<bool>` busy-flag rather than holding a
/// `MutexGuard` across the slot's lifetime, since a `MutexGuard` borrows
/// the mutex and can't be stored in a `'static`, `Send`-across-threads
/// struct without unsafe lifetime tricks. The flag is flipped back to
/// `false` on `Drop`, covering early return and panics the same way
/// `AgentSlot` does.
pub struct PromotionSlot {
    _private: (),
}

fn promotion_busy() -> &'static Mutex<bool> {
    static FLAG: OnceLock<Mutex<bool>> = OnceLock::new();
    FLAG.get_or_init(|| Mutex::new(false))
}

impl PromotionSlot {
    /// Non-blocking acquire: `Some(guard)` if no promotion is currently
    /// running, `None` if one already holds the slot.
    pub fn try_acquire() -> Option<Self> {
        let mut busy = promotion_busy().lock().unwrap_or_else(|e| e.into_inner());
        if *busy {
            return None;
        }
        *busy = true;
        Some(Self { _private: () })
    }
}

impl Drop for PromotionSlot {
    fn drop(&mut self) {
        let mut busy = promotion_busy().lock().unwrap_or_else(|e| e.into_inner());
        *busy = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    // These tests exercise a private, freestanding `Semaphore` constructed
    // directly (not the process-global singleton via `semaphore()`/
    // `AgentSlot`) so they don't interfere with each other or with any
    // other test that might touch the real global cap.
    fn test_sem(cap: usize) -> Arc<Semaphore> {
        Arc::new(Semaphore {
            state: Mutex::new(0),
            condvar: Condvar::new(),
            cap,
        })
    }

    fn acquire(sem: &Arc<Semaphore>) {
        let mut count = sem.state.lock().unwrap();
        while *count >= sem.cap {
            count = sem.condvar.wait(count).unwrap();
        }
        *count += 1;
    }

    fn release(sem: &Arc<Semaphore>) {
        let mut count = sem.state.lock().unwrap();
        *count = count.saturating_sub(1);
        sem.condvar.notify_all();
    }

    #[test]
    fn acquire_up_to_cap_blocks_the_next_and_release_unblocks_it() {
        let sem = test_sem(2);
        acquire(&sem);
        acquire(&sem);
        assert_eq!(*sem.state.lock().unwrap(), 2);

        let progressed = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let sem2 = sem.clone();
        let progressed2 = progressed.clone();
        let barrier2 = barrier.clone();
        let handle = std::thread::spawn(move || {
            barrier2.wait();
            acquire(&sem2); // Should block until the main thread releases.
            progressed2.fetch_add(1, Ordering::SeqCst);
        });

        barrier.wait();
        // Give the spawned thread a moment to reach `wait()` inside acquire.
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            progressed.load(Ordering::SeqCst),
            0,
            "third acquire must not proceed while cap=2 slots are both held"
        );

        release(&sem);
        handle.join().unwrap();
        assert_eq!(
            progressed.load(Ordering::SeqCst),
            1,
            "release must unblock the waiting acquire"
        );
        assert_eq!(*sem.state.lock().unwrap(), 2);
    }

    #[test]
    fn agent_slot_releases_on_drop_including_early_return() {
        // Use the real process-global semaphore here since AgentSlot only
        // operates on it, but keep the test order-independent by reading
        // the count delta rather than an absolute value (other tests in
        // this binary may run concurrently and briefly hold slots, though
        // none currently do in this module).
        let before = active_count();
        {
            let _slot = AgentSlot::acquire(|_| {});
            assert_eq!(active_count(), before + 1);
        }
        assert_eq!(active_count(), before);
    }

    #[test]
    fn max_concurrent_jobs_parses_env_or_falls_back_to_default() {
        // Exercise the parsing logic directly (not through the env var,
        // which is process-global and would race with other tests) by
        // replicating the same filter/parse/fallback chain.
        fn parse(raw: Option<&str>) -> usize {
            raw.and_then(|s| s.parse::<usize>().ok())
                .filter(|n| *n >= 1)
                .unwrap_or(DEFAULT_MAX_CONCURRENT_JOBS)
        }

        assert_eq!(parse(None), DEFAULT_MAX_CONCURRENT_JOBS);
        assert_eq!(parse(Some("")), DEFAULT_MAX_CONCURRENT_JOBS);
        assert_eq!(parse(Some("not a number")), DEFAULT_MAX_CONCURRENT_JOBS);
        assert_eq!(parse(Some("0")), DEFAULT_MAX_CONCURRENT_JOBS);
        assert_eq!(parse(Some("-3")), DEFAULT_MAX_CONCURRENT_JOBS);
        assert_eq!(parse(Some("3.5")), DEFAULT_MAX_CONCURRENT_JOBS);
        assert_eq!(parse(Some("5")), 5);
        assert_eq!(parse(Some("1")), 1);
    }

    #[test]
    fn promotion_slot_refuses_second_acquire_while_first_is_held() {
        let first = PromotionSlot::try_acquire();
        assert!(first.is_some(), "first acquire must succeed");
        assert!(
            PromotionSlot::try_acquire().is_none(),
            "second acquire must refuse while the first is held"
        );
        drop(first);
        assert!(
            PromotionSlot::try_acquire().is_some(),
            "acquire must succeed again after the holder drops"
        );
    }
}
