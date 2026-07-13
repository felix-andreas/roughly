//! Roughly's incremental analysis substrate: a small, generic red-green memoized-query engine.
//!
//! This crate is the analysis substrate: the **substrate only** — a reusable memoization core with no
//! R-specific logic. The R queries (parse, lower, naming, typecheck, diagnostics) are layered on top in
//! [`queries`]; see `DESIGN.md` for the architecture.
//!
//! # The model
//!
//! Computation is expressed as **queries**, each identified by a host-chosen [`QueryGroup::Key`]. There
//! are two flavors:
//!
//! - **Input queries** — set from the outside via [`Engine::set_input`] (source text, config, the stdlib
//!   stub library) and removed via [`Engine::remove_input`] (file deletion). The engine never computes
//!   these.
//! - **Derived queries** — computed by [`QueryGroup::execute`], which reads other queries through
//!   [`Engine::fetch`]. Their results are memoized.
//!
//! The host never declares dependencies. A derived body simply *reads* whatever it needs through
//! [`Engine::fetch`]; the engine records those reads and uses them to invalidate exactly and only what an
//! edit can affect.
//!
//! # The red-green algorithm
//!
//! A single global [`Revision`] counter is the engine's logical clock. It is bumped on every
//! [`Engine::set_input`]. Each memo records, in revision units, when it was last *verified* still-valid
//! (`verified_at`) and when its value last *changed* (`changed_at`), plus the list of queries it read.
//!
//! On [`Engine::fetch`] the engine validates the memo:
//!
//! 1. **Green (trivial):** if `verified_at == current revision`, the memo was already validated this
//!    revision — return the cached value.
//! 2. **Green (early cutoff):** otherwise deep-validate the recorded dependencies. If none of them
//!    *changed* after our `verified_at`, nothing we read is different — bump `verified_at` to the current
//!    revision and return the cached value **without re-running the body**.
//! 3. **Red (recompute):** some dependency changed, so re-run the body. If the new value equals the old
//!    one (value-equality), keep the old `changed_at` so the change does **not** propagate downstream
//!    (*cutoff propagation*); otherwise record `changed_at = current revision`.
//!
//! Inputs get the same treatment at the source: [`Engine::set_input`] **backdates** `changed_at` when the
//! new value equals the old, so a no-op re-set leaves every dependent green without running a single body.
//!
//! Together these give the two properties incremental analysis needs: an edit recomputes work
//! proportional to its blast radius (not the package size), and a structurally-irrelevant edit (a
//! comment, a same-length rename) stops propagating the moment a value stops changing.
//!
//! Deep validation itself is bounded by [`Durability`]: inputs declare how often they change, memos record
//! the weakest durability they transitively read, and a memo whose level saw no change since its last
//! verification is green in O(1) — after an open-document keystroke, the unopened files' chains are never
//! walked at all.
//!
//! # What lives elsewhere
//!
//! The core assumes an **acyclic** dependency graph. R has exactly one cyclic query — the package
//! interface fixed-point for mutual re-exports — and it is handled inside that query's own body (bounded
//! fixed-point iteration with `Unknown`-pinning), not by the core. Any *other* cycle is an accidental host
//! bug: a re-entered query body panics with a clear message instead of overflowing the stack. Parallelism
//! is a possible extension the current core does not implement; see `DESIGN.md`.

// The R-specific query group lives here, on top of the generic core below. The core stays analysis-free;
// only `queries` depends on the `analysis` crate.
pub mod queries;

// The engine-backed IDE view: serves `analysis::ide::generic::*` off the query graph. Depends on
// `analysis` (for the shared `IdeDatabase` orchestration) and `queries`.
pub mod ide_view;

use std::{
    any::Any,
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    fmt::Debug,
    hash::Hash,
    panic::{AssertUnwindSafe, catch_unwind, panic_any, resume_unwind},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Shared-ownership handle for memoized values. Aliased in one place so the future parallel retrofit
/// (`DESIGN.md` §6) swaps `Rc` for `Arc` here instead of pervasively. Today the engine is single-threaded;
/// `Rc` is the right cost until parallelism is added on measured evidence.
pub type Shared<T> = Rc<T>;

/// The memo table type, aliased alongside [`Shared`] as the second half of the concurrency hedge: a
/// parallel retrofit replaces this single `RefCell<HashMap<…>>` with a concurrency-safe map (sharded /
/// `RwLock` / lock-free) in one place, leaving the red-green algorithm untouched.
type MemoTable<K> = RefCell<HashMap<K, Slot<K>>>;

/// The engine's logical clock. Bumped once per [`Engine::set_input`]; memos are validated against it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Revision(u64);

/// How often an input is expected to change, the lever that makes validation sub-linear in practice.
///
/// Every input carries a durability; every derived memo records the **minimum** durability across the
/// dependencies its last recompute read (the weakest input that can reach it). The engine tracks, per
/// durability level, the last revision at which an input of that level (or a stronger one) actually
/// changed. Validation then short-circuits: a memo whose durability level has seen **no** change since the
/// memo was last verified is green without walking a single dependency edge.
///
/// The payoff is the editor's traffic shape: open-document keystrokes are [`Durability::LOW`] writes, so
/// the per-file chains of the thousands of *unopened* package files — all [`Durability::HIGH`] — validate
/// in O(1) each instead of recursing to their inputs. An all-files fold still walks its N edges (its own
/// durability is LOW: it reads the open file too), but each edge check is now O(1), turning the
/// per-keystroke validation walk from O(N·depth) into O(N) with a small constant.
///
/// Changing an input's durability is sound in both directions: a **downgrade** (HIGH → LOW, an unopened
/// file becoming an open document) refuses the value-equality backdate and counts as a change at the *old*
/// level, so every dependent that recorded HIGH deep-validates once — and that walk re-records each
/// visited memo's durability minimum, so even memos that green out by early cutoff (their values never
/// changed) pick up the weaker level. An **upgrade** (LOW → HIGH, a closed document reverting to disk)
/// keeps the backdate; dependents conservatively keep their recorded LOW until their next deep walk
/// re-mins them back up.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Durability(u8);

impl Durability {
    /// High-churn inputs: the open documents an editor mutates on every keystroke.
    pub const LOW: Durability = Durability(0);
    /// Low-churn inputs: on-disk sources not open in an editor, project membership, configuration.
    pub const HIGH: Durability = Durability(1);

    const LEVELS: usize = 2;
}

impl Revision {
    /// The clock value before any input is set. No real memo is ever verified at `START`, so it is also
    /// the safe "never changed / never seen" floor used when maximizing over an empty dependency set.
    pub const START: Revision = Revision(0);

    fn next(self) -> Revision {
        Revision(self.0 + 1)
    }
}

/// A type-erased query value paired with a same-type equality test, produced by [`Stored::new`].
///
/// Erasure (`Rc<dyn Any>`) lets the engine hold every query's value in one table regardless of type while
/// still handing typed values back through [`Engine::fetch`]. The captured comparator is what powers
/// value-equality backdating and early cutoff without the engine knowing any concrete type.
pub struct Stored {
    value: Shared<dyn Any>,
    equals: fn(&dyn Any, &dyn Any) -> bool,
}

impl Stored {
    /// Wrap a freshly computed (or freshly set) value for the engine. `T: PartialEq` is what makes
    /// early cutoff exact: two computations that produce equal `T`s do not propagate.
    pub fn new<T: Any + PartialEq>(value: T) -> Stored {
        Stored {
            value: Shared::new(value),
            equals: equals::<T>,
        }
    }
}

/// The set of queries the host defines, and how to compute the derived ones.
///
/// A host implements this once for its whole query graph: [`Key`](QueryGroup::Key) enumerates every query
/// (inputs and derived alike), and [`execute`](QueryGroup::execute) is the body dispatcher for derived
/// queries. `&self` is the place to keep host-side instrumentation (e.g. execution counters); the engine
/// hands it back through [`Engine::group`].
pub trait QueryGroup: Sized + 'static {
    /// Identifies a query. Cheap to clone and hash — it is stored on every memo and dependency edge.
    /// `Debug` is required only so the accidental-cycle guard can name the offending key.
    type Key: Clone + Eq + Hash + Debug;

    /// Compute one derived query. Read dependencies through `engine` ([`Engine::fetch`]); the engine
    /// records them automatically. Must only be called for derived keys — input keys are set externally
    /// and never reach this method.
    fn execute(&self, engine: &Engine<Self>, key: &Self::Key) -> Stored;
}

/// The memoized-query database for one [`QueryGroup`].
pub struct Engine<G: QueryGroup> {
    group: G,
    revision: Cell<Revision>,
    // `last_change[level]` is the last revision an input of durability `level` genuinely changed
    // (value-equality backdates don't count). Validation's durability fast path compares a memo's
    // `verified_at` against the maximum over the levels at or above the memo's durability.
    last_change: Cell<[Revision; Durability::LEVELS]>,
    // Validation-walk step counter (one increment per slot visited), the observable that lets tests pin
    // the durability fast path: after an open-document edit, the walk must touch the edited file's chain
    // and the fold edges, not every unopened file's chain.
    validations: Cell<u64>,
    slots: MemoTable<G::Key>,
    // One frame per in-flight derived body: the keys the body read, plus the minimum durability across
    // them (the body's own durability). `fetch` pushes the read key onto the top frame, so a body's
    // dependency list is collected at runtime with no host declaration. An empty stack means a read made
    // outside any body (a top-level fetch or internal validation), which records nothing.
    dependency_stack: RefCell<Vec<DependencyFrame<G::Key>>>,
    // The keys whose bodies are on the current `recompute` stack. Re-entering one is an accidental query
    // cycle (a derived body transitively fetching itself); without this marker that recurses until the
    // stack overflows. The one *intended* cycle (the package-interface fixed-point) is resolved inside a
    // single body and never re-enters `fetch` on its own key, so it never trips this guard.
    computing: RefCell<HashSet<G::Key>>,
    // The cancellation token of the in-flight [`Engine::fetch_cancellable`], if any. `None` for the plain
    // [`Engine::fetch`] path, which is therefore byte-for-byte unchanged: [`Engine::check_cancelled`] is a
    // no-op without an installed token. Only this `Arc<AtomicBool>` ever crosses a thread (a newer edit
    // flips it from the request thread); the engine itself stays single-threaded (`Rc`/`RefCell`).
    cancellation: RefCell<Option<Arc<AtomicBool>>>,
}

/// The sentinel returned by [`Engine::fetch_cancellable`] when an installed cancellation token was observed
/// set at a cancellation check point, so the in-flight computation was abandoned. It carries no data: the
/// caller's response is always "discard and recompute against the newer input" (the latest edit wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl<G: QueryGroup> Engine<G> {
    pub fn new(group: G) -> Engine<G> {
        Engine {
            group,
            revision: Cell::new(Revision::START),
            last_change: Cell::new([Revision::START; Durability::LEVELS]),
            validations: Cell::new(0),
            slots: RefCell::new(HashMap::new()),
            dependency_stack: RefCell::new(Vec::new()),
            computing: RefCell::new(HashSet::new()),
            cancellation: RefCell::new(None),
        }
    }

    /// The host's query group, for reading instrumentation it kept on `&self`.
    pub fn group(&self) -> &G {
        &self.group
    }

    /// The current logical clock value.
    pub fn revision(&self) -> Revision {
        self.revision.get()
    }

    /// Number of live memo slots (inputs + derived). Observability for tests and eviction.
    pub fn slot_count(&self) -> usize {
        self.slots.borrow().len()
    }

    /// Total validation-walk steps taken so far (one per slot visited by [`fetch`](Engine::fetch)'s
    /// red-green walk). The *delta* across an edit-plus-read is the walk cost the durability fast path
    /// exists to bound; the regression tests pin it.
    pub fn validation_count(&self) -> u64 {
        self.validations.get()
    }

    /// Set or replace an input at [`Durability::LOW`] — the right call for high-churn inputs (an open
    /// document's text). Low-churn inputs should use
    /// [`set_input_durable`](Engine::set_input_durable) so the durability fast path can skip their
    /// dependents' validation walks.
    pub fn set_input<T: Any + PartialEq>(&mut self, key: G::Key, value: T) {
        self.set_input_durable(key, value, Durability::LOW);
    }

    /// Set or replace an input with an explicit [`Durability`]. Bumps the revision unconditionally (it is
    /// the clock); but **backdates** `changed_at` when the new value equals the previous one, so a no-op
    /// re-set leaves every dependent green. The backdate is refused on a durability **downgrade** even for
    /// an equal value: dependents recorded the old, stronger level, and only a recompute re-records — so
    /// the downgrade must read as a change at the old level to flush those records (see [`Durability`]).
    /// Takes `&mut self` because an input mutation is the one place the database is written.
    pub fn set_input_durable<T: Any + PartialEq>(
        &mut self,
        key: G::Key,
        value: T,
        durability: Durability,
    ) {
        let revision = self.revision.get().next();
        self.revision.set(revision);
        let stored = Stored::new(value);
        let mut slots = self.slots.borrow_mut();
        let previous_durability = slots
            .get(&key)
            .filter(|previous| previous.is_input)
            .map(|previous| previous.durability);
        let backdatable = previous_durability.is_some_and(|previous| durability >= previous);
        let changed_at = match slots.get(&key) {
            Some(previous)
                if backdatable
                    && previous.is_input
                    && previous.value.as_ref().is_some_and(|previous_value| {
                        (stored.equals)(previous_value.as_ref(), stored.value.as_ref())
                    }) =>
            {
                previous.changed_at
            }
            _ => {
                // A genuine change (or a downgrade) invalidates the fast path for every level at or
                // below the strongest durability this input has carried across the transition.
                let change_level = previous_durability
                    .map_or(durability, |previous| Durability::max(previous, durability));
                let mut last_change = self.last_change.get();
                for entry in last_change.iter_mut().take(change_level.0 as usize + 1) {
                    *entry = revision;
                }
                self.last_change.set(last_change);
                revision
            }
        };
        slots.insert(
            key,
            Slot {
                value: Some(stored.value),
                verified_at: revision,
                changed_at,
                durability,
                dependencies: Vec::new(),
                is_input: true,
            },
        );
    }

    /// Remove an input, e.g. when its source file is deleted. Bumps the revision (it is a clock event) and
    /// leaves a **tombstone** — a valueless input slot marked changed at this revision — so that every
    /// dependent that recorded the input revalidates and observes it changed. The intended consumer is a
    /// query that folds over a *set* of inputs (the package-file set in the real graph): on revalidation it
    /// recomputes against the now-smaller set and never fetches the removed input. A body that fetches a
    /// removed input directly is a host bug and panics in [`fetch`]. Takes `&mut self` like [`set_input`].
    pub fn remove_input(&mut self, key: &G::Key) {
        let revision = self.revision.get().next();
        self.revision.set(revision);
        let mut slots = self.slots.borrow_mut();
        // The removal is a change at the input's own level: its HIGH dependents must fail the fast path
        // and revalidate. The tombstone itself stays LOW so anything that still records it keeps
        // deep-validating rather than fast-skipping a dead edge.
        let change_level = slots
            .get(key)
            .filter(|previous| previous.is_input)
            .map_or(Durability::LOW, |previous| previous.durability);
        let mut last_change = self.last_change.get();
        for entry in last_change.iter_mut().take(change_level.0 as usize + 1) {
            *entry = revision;
        }
        self.last_change.set(last_change);
        slots.insert(
            key.clone(),
            Slot {
                value: None,
                verified_at: revision,
                changed_at: revision,
                durability: Durability::LOW,
                dependencies: Vec::new(),
                is_input: true,
            },
        );
    }

    /// Evict every **derived** memo slot that has not been read within the last `keep_revisions`
    /// revisions, returning how many were dropped. This is the memory bound for a long editing
    /// session: derived keys are minted per file *and per symbol*, so transient names (a global typed
    /// character by character, a deleted file's queries) leave slots behind that nothing will fetch
    /// again — without eviction the table accretes forever.
    ///
    /// Correct by construction: a memo is a pure cache, so a dropped slot is simply recomputed on its
    /// next fetch ([`validate`](Engine::validate) treats a missing slot as never-computed). A live
    /// dependent that still records a dropped key revalidates through it and recomputes it — early
    /// cutoff above the recomputed key resumes as soon as a value compares equal. `verified_at` is a
    /// faithful last-used signal: validating a key green refreshes it, so anything transitively read
    /// in the window survives.
    ///
    /// **Input slots (including tombstones) are never evicted**: an input cannot be recomputed on
    /// demand — a validation walk reaching a missing input would panic — and a tombstone must outlive
    /// every stale dependency edge that still points at it.
    ///
    /// Takes `&mut self` like [`set_input`](Engine::set_input): eviction is a database write and must
    /// run between top-level fetches, never from inside a query body.
    pub fn evict_stale_memos(&mut self, keep_revisions: u64) -> usize {
        debug_assert!(
            self.dependency_stack.borrow().is_empty() && self.computing.borrow().is_empty(),
            "eviction must run between top-level fetches"
        );
        let horizon = self.revision.get().0.saturating_sub(keep_revisions);
        let mut slots = self.slots.borrow_mut();
        let before = slots.len();
        slots.retain(|_, slot| slot.is_input || slot.verified_at.0 >= horizon);
        before - slots.len()
    }

    /// Fetch a query's value, computing or validating it as needed, and record the read as a dependency
    /// of the body currently executing (if any). Panics if the stored value is not a `T` — a body asking
    /// for the wrong type for a key is a host bug, not a recoverable condition.
    pub fn fetch<T: Any>(&self, key: G::Key) -> Shared<T> {
        let value = self.fetch_any(&key);
        value
            .downcast::<T>()
            .unwrap_or_else(|_| panic!("query value type mismatch on fetch"))
    }

    /// Like [`fetch`](Engine::fetch), but returns `None` instead of panicking when `key` resolves to a
    /// removed input (a tombstone left by [`remove_input`](Engine::remove_input)). The read is still
    /// recorded as a dependency and still validated, so a dependent that reads an input which is later
    /// removed revalidates and observes it changed exactly as with [`fetch`]; it merely degrades a removed
    /// input to `None` rather than an unrecoverable panic.
    ///
    /// This is the robustness primitive behind the parse/lower boundary. The fold short-circuit in
    /// [`validate`](Engine::validate) already recomputes a file-set fold *before* any stale per-file edge is
    /// walked, so on the normal path a removed `SourceText(f)` tombstone is never reached. But that relies on
    /// an unenforced fetch *order* invariant — a future reorder that validated a stale `Lower(f)`/`Parse(f)`
    /// edge first would walk into the tombstone and panic. Reading the source through `fetch_optional` makes
    /// that walk degrade to an empty parse/module (a cutoff), not a crash.
    pub fn fetch_optional<T: Any>(&self, key: G::Key) -> Option<Shared<T>> {
        self.fetch_any_optional(&key).map(|value| {
            value
                .downcast::<T>()
                .unwrap_or_else(|_| panic!("query value type mismatch on fetch"))
        })
    }

    /// Like [`fetch`](Engine::fetch), but cooperatively cancellable: `token` is installed for the duration
    /// of this fetch, and every [`recompute`](Engine::recompute) entry (plus any host check point, e.g. the
    /// interface fixed-point's round boundaries via [`check_cancelled`](Engine::check_cancelled)) observes
    /// it. If it is set, the in-flight computation is abandoned and `Err(`[`Cancelled`]`)` is returned.
    ///
    /// **Why unwind, not a `Result` threaded through every body.** A query body reads through the infallible
    /// `fetch` and uses the returned value directly; making cancellation a `Result` would force every body
    /// (and `fetch`'s signature) to thread and `?`-propagate it, the opposite of additive. Instead the check
    /// raises a [`Cancelled`] panic that unwinds the `fetch` stack — exactly the "sentinel that unwinds"
    /// shape the design calls for. Because a memo slot is written only at the *end* of `recompute`, an
    /// abandoned pass commits **no partial memo**; the only transient state to restore is the dependency
    /// stack and the `computing` set, which this one catch point clears, leaving the engine consistent for
    /// the next fetch. A non-`Cancelled` panic (e.g. the accidental-cycle guard) is re-raised unchanged, so
    /// cancellation is strictly additive — it never masks a real bug.
    ///
    /// The raised [`Cancelled`] still runs the process panic hook. A host that cancels routinely (per
    /// keystroke) installs a hook that ignores the [`Cancelled`] payload once at startup; the tests do this
    /// around the cancellation cases.
    pub fn fetch_cancellable<T: Any>(
        &self,
        key: G::Key,
        token: Arc<AtomicBool>,
    ) -> Result<Shared<T>, Cancelled> {
        self.with_cancellation(token, || self.fetch::<T>(key))
    }

    /// Run `body` with `token` installed as the cancellation token, so every `fetch` it performs is
    /// cooperatively cancellable: a token observed set at a [`check_cancelled`](Engine::check_cancelled)
    /// point abandons the in-flight computation by unwinding the [`Cancelled`] sentinel, caught here.
    /// Generalizes [`fetch_cancellable`](Engine::fetch_cancellable) from one query to a whole multi-fetch
    /// operation (e.g. an IDE feature that primes several queries), so a host can make an entire read
    /// cancellable rather than just a single query. `Err(`[`Cancelled`]`)` means the token was observed
    /// set; a non-`Cancelled` panic is re-raised unchanged.
    pub fn with_cancellation<R>(
        &self,
        token: Arc<AtomicBool>,
        body: impl FnOnce() -> R,
    ) -> Result<R, Cancelled> {
        let previous = self.cancellation.borrow_mut().replace(token);
        let outcome = catch_unwind(AssertUnwindSafe(body));
        *self.cancellation.borrow_mut() = previous;
        match outcome {
            Ok(value) => Ok(value),
            Err(payload) if payload.is::<Cancelled>() => {
                // Abandon cleanly: drop the dependency-stack frames and `computing` markers the unwound
                // ancestor recomputes left behind. No slot was committed (the panic fires before any
                // recompute reaches its slot write), so the memo table is already consistent.
                self.dependency_stack.borrow_mut().clear();
                self.computing.borrow_mut().clear();
                Err(Cancelled)
            }
            Err(payload) => resume_unwind(payload),
        }
    }

    fn fetch_any(&self, key: &G::Key) -> Shared<dyn Any> {
        self.fetch_any_optional(key)
            .unwrap_or_else(|| panic!("fetched query {key:?} has no value (a removed input?)"))
    }

    // Record the read, validate, and return the stored value — or `None` when the slot is a tombstone (a
    // removed input). Shared by the panicking [`fetch_any`] and the tombstone-tolerant [`fetch_optional`].
    fn fetch_any_optional(&self, key: &G::Key) -> Option<Shared<dyn Any>> {
        if let Some(frame) = self.dependency_stack.borrow_mut().last_mut() {
            frame.reads.push(key.clone());
        }
        self.validate(key);
        let slots = self.slots.borrow();
        let slot = slots.get(key)?;
        // Fold the read's durability into the in-flight body (post-validation, so a recomputed derived
        // slot contributes its freshly re-recorded level).
        if let Some(frame) = self.dependency_stack.borrow_mut().last_mut() {
            frame.durability = Durability::min(frame.durability, slot.durability);
        }
        slot.value.as_ref().map(Shared::clone)
    }

    /// Validate one query against the current revision and return its `changed_at`. This is the red-green
    /// core: green-by-revision, green-by-early-cutoff, or red (recompute). Recursion walks dependencies;
    /// it does not record them (validation is not a body), and the recorded list is always acyclic.
    ///
    /// Every deepening of the fetch spine passes through here — both the dependency walk below and a
    /// recompute whose body fetches (body → `fetch` → `validate` → `recompute` → body …) — so this is
    /// the one place the host stack grows with dependency-chain depth. Query depth tracks the *user's
    /// program* (a re-export chain `a <- b <- … <- concrete` is one query level per link), which no
    /// fixed stack can bound, so within `RED_ZONE` bytes of overflow the stack is grown by
    /// `STACK_PER_GROWTH` instead: a mechanically generated deep chain slows down, it does not abort
    /// the process. On platforms where the remaining stack cannot be measured `maybe_grow` degrades
    /// to a plain call.
    fn validate(&self, key: &G::Key) -> Revision {
        // The red zone must cover every frame a body can push between two `validate` entries. That is
        // bounded by one file's inference descending to a global reference, which the lowering
        // recursion cap bounds at 160 expression frames — 512 KiB holds comfortably even for debug
        // frame sizes.
        const RED_ZONE: usize = 512 * 1024;
        const STACK_PER_GROWTH: usize = 8 * 1024 * 1024;
        stacker::maybe_grow(RED_ZONE, STACK_PER_GROWTH, || self.validate_inner(key))
    }

    fn validate_inner(&self, key: &G::Key) -> Revision {
        self.validations.set(self.validations.get() + 1);
        let revision = self.revision.get();
        let snapshot = self.slots.borrow().get(key).map(|slot| {
            (
                slot.is_input,
                slot.verified_at,
                slot.changed_at,
                slot.durability,
            )
        });

        let Some((is_input, verified_at, changed_at, durability)) = snapshot else {
            // No slot: a derived query never computed before. (Inputs are always set before first read.)
            return self.recompute(key);
        };

        if is_input || verified_at == revision {
            return changed_at;
        }

        // Durability fast path: no input at this memo's level (or below) has changed since it was last
        // verified, so nothing it transitively reads can differ — green without walking an edge.
        if self.last_change.get()[durability.0 as usize] <= verified_at {
            if let Some(slot) = self.slots.borrow_mut().get_mut(key) {
                slot.verified_at = revision;
            }
            return changed_at;
        }

        // Validate dependencies in recorded order and **short-circuit**: the moment one is found to have
        // changed after our last verification, recompute immediately without validating the rest. This is
        // not merely an optimization — it is required for correct input *removal*. A fold over a file set
        // records `ProjectFiles` first, then a per-file edge to each member's derived chain
        // (`ExportedNames(f)` → `Lower(f)` → `Parse(f)` → `SourceText(f)`). When a file is deleted,
        // `ProjectFiles` changes and its `SourceText` becomes a tombstone. Validating the fold's *stale*
        // per-file edge would walk into `Lower(f)`/`Parse(f)`, find their `SourceText(f)` tombstone
        // changed, and **recompute** them — and `Parse(f)`'s body fetches the now-absent `SourceText(f)`,
        // which panics. Short-circuiting on the already-changed `ProjectFiles` (recorded first) recomputes
        // the fold before any dead per-file edge is validated; the recompute then folds the smaller file
        // set and drops the edge. The cutoff decision is unaffected: we recompute iff some dependency
        // changed after `verified_at`, exactly as taking the max would decide.
        //
        // The walk is index-based, re-borrowing the slot table and cloning ONE dependency key per
        // step, because the borrow cannot be held across the recursive `validate` (a dependency's
        // recompute mutates the table). Cloning the whole dependency vector up front instead would
        // allocate per deep-validated slot — and for a high fan-in fold (an all-files index with
        // O(package) dependencies) that one clone is O(package) by itself. The list cannot change
        // under the walk: only `recompute(key)` rewrites it, and re-entering `key` from its own
        // dependency chain is a cycle the accidental-cycle guard already rejects.
        let mut dependency_index = 0;
        let mut walked_durability = Durability::HIGH;
        loop {
            let dependency = {
                let slots = self.slots.borrow();
                let Some(slot) = slots.get(key) else {
                    break;
                };
                match slot.dependencies.get(dependency_index) {
                    Some(dependency) => dependency.clone(),
                    None => break,
                }
            };
            if self.validate(&dependency) > verified_at {
                return self.recompute(key);
            }
            if let Some(dependency_slot) = self.slots.borrow().get(&dependency) {
                walked_durability = Durability::min(walked_durability, dependency_slot.durability);
            }
            dependency_index += 1;
        }
        // Early cutoff: nothing we read has changed since we were last verified. The walk visited
        // every dependency, so re-record the durability minimum alongside the verification: this is
        // what propagates a dependency's durability *transition* through memos whose values never
        // change. Without it, an equal-value downgrade (a file being opened re-records its chain's
        // bottom as LOW while the memos above green out by cutoff) would leave stale HIGH records
        // here, and the next LOW edit would be invisible to the fast path — a stale read.
        if let Some(slot) = self.slots.borrow_mut().get_mut(key) {
            slot.verified_at = revision;
            slot.durability = walked_durability;
        }
        changed_at
    }

    /// Whether `key`'s body is currently on the recompute stack. A query body uses this to break a
    /// genuine *domain* dependency cycle it would otherwise re-enter (the general package-interface
    /// fixed-point: mutually-referencing globals), bootstrapping the re-entrant edge to a base value
    /// instead of recursing — the same role the cyclic-query body plays for re-export cycles, but checked
    /// cooperatively rather than by tripping the accidental-cycle guard. A body that does *not* break the
    /// cycle still hits the loud [`recompute`] panic, so this never masks an unintended cycle.
    pub fn is_computing(&self, key: &G::Key) -> bool {
        self.computing.borrow().contains(key)
    }

    /// Cooperative cancellation check point. If a token is installed (a [`fetch_cancellable`](Engine::fetch_cancellable)
    /// is in flight) and observed set, abandon the computation by unwinding with the [`Cancelled`] sentinel,
    /// which `fetch_cancellable` catches. A no-op when no token is installed, so the plain `fetch` path is
    /// unaffected. `recompute` calls it at every body entry; a host whose body loops without re-entering
    /// `recompute` (the package-interface fixed-point's round loop) calls it at its own safe points so a
    /// long pass still abandons promptly. The token is read and the borrow released *before* unwinding, so
    /// no `RefCell` borrow is held across the panic.
    pub fn check_cancelled(&self) {
        let cancelled = self
            .cancellation
            .borrow()
            .as_ref()
            .is_some_and(|token| token.load(Ordering::Relaxed));
        if cancelled {
            panic_any(Cancelled);
        }
    }

    /// Depth of the in-flight dependency-recording stack: one frame per `recompute` body currently on the
    /// call stack. Zero between top-level fetches — observability for asserting an abandoned
    /// `fetch_cancellable` left the engine's transient state consistent.
    pub fn dependency_stack_depth(&self) -> usize {
        self.dependency_stack.borrow().len()
    }

    /// Number of keys whose bodies are currently on the `recompute` stack. Zero between top-level fetches;
    /// the companion to [`dependency_stack_depth`](Engine::dependency_stack_depth) for the same assertion.
    pub fn computing_count(&self) -> usize {
        self.computing.borrow().len()
    }

    fn recompute(&self, key: &G::Key) -> Revision {
        // Cooperative cancellation check point. Placed *before* any transient state is touched, so an
        // abandoned recompute adds nothing of its own to clean up — only the ancestor recomputes whose
        // bodies are mid-flight have pushed frames / `computing` entries, and the single
        // `fetch_cancellable` catch point clears those. With no token installed this is a cheap `None`
        // check that never unwinds, so the plain `fetch` path is unchanged.
        self.check_cancelled();
        let revision = self.revision.get();
        if !self.computing.borrow_mut().insert(key.clone()) {
            panic!("query cycle detected: {key:?} is already being computed");
        }
        self.dependency_stack
            .borrow_mut()
            .push(DependencyFrame::new());
        let stored = self.group.execute(self, key);
        let frame = self
            .dependency_stack
            .borrow_mut()
            .pop()
            .unwrap_or_else(DependencyFrame::new);
        self.computing.borrow_mut().remove(key);

        let mut slots = self.slots.borrow_mut();
        // Cutoff propagation: an equal recompute keeps the old `changed_at`, so consumers stay green.
        let changed_at = match slots.get(key) {
            Some(previous)
                if previous.value.as_ref().is_some_and(|previous_value| {
                    (stored.equals)(previous_value.as_ref(), stored.value.as_ref())
                }) =>
            {
                previous.changed_at
            }
            _ => revision,
        };
        slots.insert(
            key.clone(),
            Slot {
                value: Some(stored.value),
                verified_at: revision,
                changed_at,
                durability: frame.durability,
                dependencies: frame.reads,
                is_input: false,
            },
        );
        changed_at
    }
}

struct Slot<K> {
    // `None` is a tombstone left by [`Engine::remove_input`]: the input is gone but the slot is retained so
    // dependents that recorded it still validate (it reads as changed at the removal revision) instead of
    // recursing into a recompute of a now-absent input. A tombstone is never fetched for its value — a body
    // that fetches a removed input is a host bug and panics.
    value: Option<Shared<dyn Any>>,
    verified_at: Revision,
    changed_at: Revision,
    // For an input: host-declared. For a derived memo: the minimum across the dependencies its last
    // recompute read — the weakest input that can transitively reach it, which is what the validation
    // fast path checks `last_change` against.
    durability: Durability,
    dependencies: Vec<K>,
    is_input: bool,
}

// One in-flight derived body's read record: the keys it fetched, and the running minimum of their
// durabilities (which becomes the memo's own durability at the slot write).
struct DependencyFrame<K> {
    reads: Vec<K>,
    durability: Durability,
}

impl<K> DependencyFrame<K> {
    fn new() -> DependencyFrame<K> {
        DependencyFrame {
            reads: Vec::new(),
            // A body that reads nothing can never be invalidated by an input; min() over its reads then
            // correctly leaves the strongest level in place.
            durability: Durability::HIGH,
        }
    }
}

fn equals<T: Any + PartialEq>(left: &dyn Any, right: &dyn Any) -> bool {
    match (left.downcast_ref::<T>(), right.downcast_ref::<T>()) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}
