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
//!   stub library). The engine never computes these.
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
//! fixed-point iteration with `Unknown`-pinning), not by the core. Cancellation, parallelism, and memo
//! eviction are designed for but not implemented in this phase; see `DESIGN.md`.

use std::{
    any::Any,
    cell::{Cell, RefCell},
    collections::HashMap,
    hash::Hash,
    rc::Rc,
};

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
    value: Rc<dyn Any>,
    equals: fn(&dyn Any, &dyn Any) -> bool,
}

impl Stored {
    /// Wrap a freshly computed (or freshly set) value for the engine. `T: PartialEq` is what makes
    /// early cutoff exact: two computations that produce equal `T`s do not propagate.
    pub fn new<T: Any + PartialEq>(value: T) -> Stored {
        Stored {
            value: Rc::new(value),
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
    type Key: Clone + Eq + Hash;

    /// Compute one derived query. Read dependencies through `engine` ([`Engine::fetch`]); the engine
    /// records them automatically. Must only be called for derived keys — input keys are set externally
    /// and never reach this method.
    fn execute(&self, engine: &Engine<Self>, key: &Self::Key) -> Stored;
}

/// The memoized-query database for one [`QueryGroup`].
pub struct Engine<G: QueryGroup> {
    group: G,
    revision: Cell<Revision>,
    slots: RefCell<HashMap<G::Key, Slot<G::Key>>>,
    // One frame per in-flight derived body. `fetch` pushes the read key onto the top frame, so a body's
    // dependency list is collected at runtime with no host declaration. An empty stack means a read made
    // outside any body (a top-level fetch or internal validation), which records nothing.
    dependency_stack: RefCell<Vec<Vec<G::Key>>>,
}

impl<G: QueryGroup> Engine<G> {
    pub fn new(group: G) -> Engine<G> {
        Engine {
            group,
            revision: Cell::new(Revision::START),
            slots: RefCell::new(HashMap::new()),
            dependency_stack: RefCell::new(Vec::new()),
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
                    && (stored.equals)(previous.value.as_ref(), stored.value.as_ref()) =>
            {
                previous.changed_at
            }
            _ => revision,
        };
        slots.insert(
            key,
            Slot {
                value: stored.value,
                verified_at: revision,
                changed_at,
                dependencies: Vec::new(),
                is_input: true,
            },
        );
    }

    /// Fetch a query's value, computing or validating it as needed, and record the read as a dependency
    /// of the body currently executing (if any). Panics if the stored value is not a `T` — a body asking
    /// for the wrong type for a key is a host bug, not a recoverable condition.
    pub fn fetch<T: Any>(&self, key: G::Key) -> Rc<T> {
        let value = self.fetch_any(&key);
        value
            .downcast::<T>()
            .unwrap_or_else(|_| panic!("query value type mismatch on fetch"))
    }

    fn fetch_any(&self, key: &G::Key) -> Rc<dyn Any> {
        if let Some(frame) = self.dependency_stack.borrow_mut().last_mut() {
            frame.push(key.clone());
        }
        self.validate(key);
        self.slots
            .borrow()
            .get(key)
            .map(|slot| Rc::clone(&slot.value))
            .expect("slot present after validate")
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

        let max_dependency_change = dependencies
            .iter()
            .map(|dependency| self.validate(dependency))
            .max()
            .unwrap_or(Revision::START);
        if max_dependency_change <= verified_at {
            // Early cutoff: nothing we read has changed since we were last verified.
            if let Some(slot) = self.slots.borrow_mut().get_mut(key) {
                slot.verified_at = revision;
            }
            return changed_at;
        }

        self.recompute(key)
    }

    fn recompute(&self, key: &G::Key) -> Revision {
        let revision = self.revision.get();
        self.dependency_stack.borrow_mut().push(Vec::new());
        let stored = self.group.execute(self, key);
        let dependencies = self.dependency_stack.borrow_mut().pop().unwrap_or_default();

        let mut slots = self.slots.borrow_mut();
        // Cutoff propagation: an equal recompute keeps the old `changed_at`, so consumers stay green.
        let changed_at = match slots.get(key) {
            Some(previous)
                if (stored.equals)(previous.value.as_ref(), stored.value.as_ref()) =>
            {
                previous.changed_at
            }
            _ => revision,
        };
        slots.insert(
            key.clone(),
            Slot {
                value: stored.value,
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
    value: Rc<dyn Any>,
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
