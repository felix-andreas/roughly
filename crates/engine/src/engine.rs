//! Roughly's incremental analysis substrate: a small, generic red-green memoized-query engine.
//!
//! This crate is the foundation of the analysis-engine rewrite (decision record
//! `.agents/decisions/incremental-architecture-and-recheck.md`). It is the **substrate only** — a
//! reusable memoization core with no R-specific logic. The R queries (parse, lower, naming, typecheck,
//! diagnostics) are layered on top in later phases; see `DESIGN.md` for the full plan.
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
//! Together these give the two properties the rewrite needs: an edit recomputes work proportional to its
//! blast radius (not the package size), and a structurally-irrelevant edit (a comment, a same-length
//! rename) stops propagating the moment a value stops changing.
//!
//! # What lives elsewhere
//!
//! The core assumes an **acyclic** dependency graph. R has exactly one cyclic query — the package
//! interface fixed-point for mutual re-exports — and it is handled inside that query's own body (bounded
//! fixed-point iteration with `Unknown`-pinning), not by the core. Any *other* cycle is an accidental host
//! bug: a re-entered query body panics with a clear message instead of overflowing the stack. Cancellation,
//! parallelism, and memo eviction are designed for but not implemented in this phase; see `DESIGN.md`.

// The R-specific query group lives here, on top of the generic core below. The core stays analysis-free;
// only `queries` depends on the `analysis` crate.
pub mod queries;

use std::{
    any::Any,
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    fmt::Debug,
    hash::Hash,
    rc::Rc,
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
pub struct Revision(u32);

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
    slots: MemoTable<G::Key>,
    // One frame per in-flight derived body. `fetch` pushes the read key onto the top frame, so a body's
    // dependency list is collected at runtime with no host declaration. An empty stack means a read made
    // outside any body (a top-level fetch or internal validation), which records nothing.
    dependency_stack: RefCell<Vec<Vec<G::Key>>>,
    // The keys whose bodies are on the current `recompute` stack. Re-entering one is an accidental query
    // cycle (a derived body transitively fetching itself); without this marker that recurses until the
    // stack overflows. The one *intended* cycle (the package-interface fixed-point) is resolved inside a
    // single body and never re-enters `fetch` on its own key, so it never trips this guard.
    computing: RefCell<HashSet<G::Key>>,
}

impl<G: QueryGroup> Engine<G> {
    pub fn new(group: G) -> Engine<G> {
        Engine {
            group,
            revision: Cell::new(Revision::START),
            slots: RefCell::new(HashMap::new()),
            dependency_stack: RefCell::new(Vec::new()),
            computing: RefCell::new(HashSet::new()),
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

    /// Set or replace an input. Bumps the revision unconditionally (it is the clock); but **backdates**
    /// `changed_at` when the new value equals the previous one, so a no-op re-set leaves every dependent
    /// green. Takes `&mut self` because an input mutation is the one place the database is written.
    pub fn set_input<T: Any + PartialEq>(&mut self, key: G::Key, value: T) {
        let revision = self.revision.get().next();
        self.revision.set(revision);
        let stored = Stored::new(value);
        let mut slots = self.slots.borrow_mut();
        let changed_at = match slots.get(&key) {
            Some(previous)
                if previous.is_input
                    && previous.value.as_ref().is_some_and(|previous_value| {
                        (stored.equals)(previous_value.as_ref(), stored.value.as_ref())
                    }) =>
            {
                previous.changed_at
            }
            _ => revision,
        };
        slots.insert(
            key,
            Slot {
                value: Some(stored.value),
                verified_at: revision,
                changed_at,
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
        self.slots.borrow_mut().insert(
            key.clone(),
            Slot {
                value: None,
                verified_at: revision,
                changed_at: revision,
                dependencies: Vec::new(),
                is_input: true,
            },
        );
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

    fn fetch_any(&self, key: &G::Key) -> Shared<dyn Any> {
        if let Some(frame) = self.dependency_stack.borrow_mut().last_mut() {
            frame.push(key.clone());
        }
        self.validate(key);
        self.slots
            .borrow()
            .get(key)
            .and_then(|slot| slot.value.as_ref().map(Shared::clone))
            .unwrap_or_else(|| panic!("fetched query {key:?} has no value (a removed input?)"))
    }

    /// Validate one query against the current revision and return its `changed_at`. This is the red-green
    /// core: green-by-revision, green-by-early-cutoff, or red (recompute). Recursion walks dependencies;
    /// it does not record them (validation is not a body), and the recorded list is always acyclic.
    fn validate(&self, key: &G::Key) -> Revision {
        let revision = self.revision.get();
        let snapshot = self.slots.borrow().get(key).map(|slot| {
            (
                slot.is_input,
                slot.verified_at,
                slot.changed_at,
                slot.dependencies.clone(),
            )
        });

        let Some((is_input, verified_at, changed_at, dependencies)) = snapshot else {
            // No slot: a derived query never computed before. (Inputs are always set before first read.)
            return self.recompute(key);
        };

        if is_input || verified_at == revision {
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
        for dependency in &dependencies {
            if self.validate(dependency) > verified_at {
                return self.recompute(key);
            }
        }
        // Early cutoff: nothing we read has changed since we were last verified.
        if let Some(slot) = self.slots.borrow_mut().get_mut(key) {
            slot.verified_at = revision;
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

    fn recompute(&self, key: &G::Key) -> Revision {
        let revision = self.revision.get();
        if !self.computing.borrow_mut().insert(key.clone()) {
            panic!("query cycle detected: {key:?} is already being computed");
        }
        self.dependency_stack.borrow_mut().push(Vec::new());
        let stored = self.group.execute(self, key);
        let dependencies = self.dependency_stack.borrow_mut().pop().unwrap_or_default();
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
                dependencies,
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
    dependencies: Vec<K>,
    is_input: bool,
}

fn equals<T: Any + PartialEq>(left: &dyn Any, right: &dyn Any) -> bool {
    match (left.downcast_ref::<T>(), right.downcast_ref::<T>()) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}
