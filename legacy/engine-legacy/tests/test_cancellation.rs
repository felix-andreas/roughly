//! Cooperative cancellation (`DESIGN.md` §6): single-engine, off-thread, "latest edit wins".
//!
//! The model: the engine lives on one thread; a newer edit (from another thread, or simply the next
//! request) flips an `Arc<AtomicBool>` cancellation token, and the in-flight cross-file pass abandons
//! cheaply so the newest input is the one that gets computed. **Only the token crosses the thread
//! boundary** — the engine itself stays single-threaded (`Rc`/`RefCell`). On cancellation the in-flight
//! `fetch_cancellable` returns `Err(Cancelled)` with the engine left consistent (nothing computing, no
//! dangling dependency frame, no partial memo committed), and a subsequent fetch with the token cleared
//! computes the correct up-to-date result.
//!
//! These tests also pin the additivity guarantee: the accidental-cycle guard still fires under a
//! cancellable fetch (a `Cancelled` unwind never masks a real cycle), and the plain `fetch` path (no token)
//! is unchanged.

use {
    engine::{Cancelled, Engine, QueryGroup, Stored},
    std::{
        panic::{AssertUnwindSafe, catch_unwind},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        thread,
    },
};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Key {
    Source,
    // A derived query whose body does `work_units` units of "work", calling `engine.check_cancelled()`
    // on every unit — the cooperative check point. Its value is `Source * 2`, so a newer `Source` edit
    // changes the result and we can prove the *post-cancellation* fetch recomputed against the new input.
    Slow,
    // A two-node cycle for the additive accidental-cycle-guard assertion.
    Ping,
    Pong,
}

#[derive(Default)]
struct Queries {
    // Flipped true by the `Slow` body the moment it starts, so the canceller thread can wait for the
    // in-flight pass to actually be running before it flips the token (no "cancel before start" race).
    started: Arc<AtomicBool>,
    // How many cooperative-check units `Slow` performs. The cancellation case sets it high enough that the
    // canceller always wins; the uncancelled cases set it tiny so the body completes immediately.
    work_units: AtomicU64,
    // How many times `Slow` ran to completion — must stay 0 across a cancelled pass (it never committed).
    slow_completions: AtomicU64,
}

impl QueryGroup for Queries {
    type Key = Key;

    fn execute(&self, engine: &Engine<Self>, key: &Key) -> Stored {
        match key {
            Key::Source => panic!("input queries are never executed"),
            Key::Slow => {
                self.started.store(true, Ordering::Relaxed);
                let source = engine.fetch::<u64>(Key::Source);
                let units = self.work_units.load(Ordering::Relaxed);
                for _ in 0..units {
                    // The cooperative check: a no-op until the installed token is set, then it unwinds.
                    engine.check_cancelled();
                }
                self.slow_completions.fetch_add(1, Ordering::Relaxed);
                Stored::new(*source * 2)
            }
            Key::Ping => Stored::new(*engine.fetch::<u64>(Key::Pong)),
            Key::Pong => Stored::new(*engine.fetch::<u64>(Key::Ping)),
        }
    }
}

// Install a panic hook that swallows the `Cancelled` sentinel's output (cancellation is routine — a host
// that cancels per keystroke installs this once at startup) while delegating every other panic to the
// previous hook so genuine failures still surface. Left installed for the rest of this test binary; since
// it only suppresses `Cancelled`, it cannot hide a real panic.
fn silence_cancelled_panics() {
    let previous = Arc::new(std::panic::take_hook());
    std::panic::set_hook(Box::new(move |info| {
        if !info.payload().is::<Cancelled>() {
            previous(info);
        }
    }));
}

// The headline: a newer edit flips the token mid-flight, the in-flight long pass abandons cleanly, the
// engine is left consistent, and the next fetch computes the up-to-date result.
#[test]
fn latest_edit_wins_with_cooperative_cancellation() {
    silence_cancelled_panics();

    let mut engine = Engine::new(Queries::default());
    engine.set_input(Key::Source, 10u64);
    // High enough that the canceller always flips the token before the loop can finish; if cancellation
    // were broken the body would *complete* (failing the assertions below) rather than hang the suite.
    engine
        .group()
        .work_units
        .store(1_000_000_000, Ordering::Relaxed);
    engine.group().started.store(false, Ordering::Relaxed);

    // The "newer edit" arrives on another thread. Only the token (an `Arc<AtomicBool>`) crosses the
    // boundary; the engine never leaves this thread. The thread waits until the in-flight `Slow` body is
    // actually running, then flips the token.
    let token = Arc::new(AtomicBool::new(false));
    let token_for_thread = token.clone();
    let started = engine.group().started.clone();
    let canceller = thread::spawn(move || {
        while !started.load(Ordering::Relaxed) {
            std::hint::spin_loop();
        }
        token_for_thread.store(true, Ordering::Relaxed);
    });

    let outcome = engine.fetch_cancellable::<u64>(Key::Slow, token.clone());
    canceller.join().expect("canceller thread should join");

    assert_eq!(
        outcome,
        Err(Cancelled),
        "the in-flight long pass abandons when the newer edit flips the token"
    );
    assert_eq!(
        engine.group().slow_completions.load(Ordering::Relaxed),
        0,
        "the abandoned body never ran to completion"
    );
    // The engine's transient state is consistent: nothing left computing, no dangling dependency frame.
    assert_eq!(
        engine.computing_count(),
        0,
        "the computing set is empty after a cancelled fetch"
    );
    assert_eq!(
        engine.dependency_stack_depth(),
        0,
        "the dependency stack is empty after a cancelled fetch"
    );
    // No partial memo was committed: the only slot is the `Source` input — `Slow` was never stored.
    assert_eq!(
        engine.slot_count(),
        1,
        "no partial `Slow` memo was committed by the abandoned pass"
    );

    // The newer edit lands and the token is cleared. The next fetch recomputes against the new input and
    // returns the correct, up-to-date result — the latest edit wins.
    engine.set_input(Key::Source, 21u64);
    engine.group().work_units.store(4, Ordering::Relaxed);
    token.store(false, Ordering::Relaxed);
    let value = engine
        .fetch_cancellable::<u64>(Key::Slow, token.clone())
        .expect("the post-edit fetch is not cancelled");
    assert_eq!(
        *value, 42,
        "the subsequent fetch computes the up-to-date result (21 * 2)"
    );
    assert_eq!(
        engine.group().slow_completions.load(Ordering::Relaxed),
        1,
        "`Slow` completed exactly once, on the post-edit fetch"
    );

    // And a re-fetch is served from the now-warm memo (cancellation did not corrupt memoization).
    let again = engine
        .fetch_cancellable::<u64>(Key::Slow, token)
        .expect("the warm fetch is not cancelled");
    assert_eq!(*again, 42);
    assert_eq!(
        engine.group().slow_completions.load(Ordering::Relaxed),
        1,
        "the warm re-fetch served the memo without recomputing"
    );
}

// The LSP worker makes a whole multi-fetch READ cancellable (an IDE feature primes several queries) via
// `with_cancellation`, not a single `fetch_cancellable`. Same headline guarantee through that primitive:
// a newer edit flips the token mid-read, the in-flight multi-step pass abandons cleanly, and the next
// read computes the up-to-date result — the exact shape `EngineWorker::cancellable` relies on.
#[test]
fn with_cancellation_latest_edit_wins_for_a_multi_fetch_read() {
    silence_cancelled_panics();

    let mut engine = Engine::new(Queries::default());
    engine.set_input(Key::Source, 10u64);
    engine
        .group()
        .work_units
        .store(1_000_000_000, Ordering::Relaxed);
    engine.group().started.store(false, Ordering::Relaxed);

    let token = Arc::new(AtomicBool::new(false));
    let token_for_thread = token.clone();
    let started = engine.group().started.clone();
    let canceller = thread::spawn(move || {
        while !started.load(Ordering::Relaxed) {
            std::hint::spin_loop();
        }
        token_for_thread.store(true, Ordering::Relaxed);
    });

    // A read closure that touches the engine more than once under one installed token (the worker's IDE-
    // read shape). The long `Slow` pass abandons the moment the canceller flips the token mid-flight.
    let outcome = engine.with_cancellation(token.clone(), || {
        let _warm = engine.fetch::<u64>(Key::Source);
        *engine.fetch::<u64>(Key::Slow)
    });
    canceller.join().expect("canceller thread should join");

    assert_eq!(
        outcome,
        Err(Cancelled),
        "the multi-fetch read abandons when the newer edit flips the token"
    );
    assert_eq!(
        engine.group().slow_completions.load(Ordering::Relaxed),
        0,
        "the abandoned read never ran to completion"
    );
    assert_eq!(
        engine.computing_count(),
        0,
        "nothing left computing after a cancelled read"
    );
    assert_eq!(
        engine.dependency_stack_depth(),
        0,
        "no dangling dependency frame after a cancelled read"
    );

    // Newer edit + cleared token: the next read computes the up-to-date result. Latest edit wins.
    engine.set_input(Key::Source, 21u64);
    engine.group().work_units.store(4, Ordering::Relaxed);
    token.store(false, Ordering::Relaxed);
    let value = engine
        .with_cancellation(token, || *engine.fetch::<u64>(Key::Slow))
        .expect("the post-edit read is not cancelled");
    assert_eq!(
        value, 42,
        "the read after the edit computes the up-to-date result (21 * 2)"
    );
    assert_eq!(
        engine.group().slow_completions.load(Ordering::Relaxed),
        1,
        "`Slow` completed exactly once, on the post-edit read"
    );
}

// Additive guarantee #1: the accidental-cycle guard still fires under a cancellable fetch. A `Cancelled`
// unwind is caught and converted to `Err`, but any *other* panic — here the cycle guard — is re-raised
// unchanged, so cancellation never masks a real host bug.
#[test]
fn cycle_guard_still_fires_under_a_cancellable_fetch() {
    silence_cancelled_panics();
    let engine = Engine::new(Queries::default());
    // The token is installed but never set, so cancellation is inert; the cycle must still be caught.
    let token = Arc::new(AtomicBool::new(false));
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = engine.fetch_cancellable::<u64>(Key::Ping, token);
    }))
    .expect_err("a cyclic query must still panic, even through a cancellable fetch");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains("query cycle detected") && message.contains("Ping"),
        "the cycle guard, not cancellation, named the failure: {message:?}"
    );
}

// Additive guarantee #2: with no token, the plain `fetch` path is byte-for-byte unchanged, and a
// `fetch_cancellable` whose token is never set behaves exactly like `fetch`.
#[test]
fn cancellation_is_additive_when_no_token_is_set() {
    let mut engine = Engine::new(Queries::default());
    engine.set_input(Key::Source, 7u64);
    engine.group().work_units.store(3, Ordering::Relaxed);

    assert_eq!(
        *engine.fetch::<u64>(Key::Slow),
        14,
        "the plain fetch path is unaffected"
    );

    let token = Arc::new(AtomicBool::new(false));
    let value = engine
        .fetch_cancellable::<u64>(Key::Slow, token)
        .expect("an unset token never cancels");
    assert_eq!(
        *value, 14,
        "fetch_cancellable with an inert token matches plain fetch"
    );
    assert_eq!(engine.computing_count(), 0);
    assert_eq!(engine.dependency_stack_depth(), 0);
}
