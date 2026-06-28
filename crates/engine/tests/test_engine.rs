// Smoke tests for the generic red-green substrate, ported from the de-risking spike's correctness checks
// (`crates/analysis/tests/test_query_spike.rs`) onto a trivial query graph defined entirely here:
//
//   Text  (input)  ->  Length  (derived: text.len())  ->  Doubled (derived: length * 2)
//   Other (input)  ->  OtherLength (derived: other.len())   [an independent chain]
//
// The two chains share no inputs, so editing one must not recompute the other — this is what proves
// dependency recording (no hand-declared deps). Body-execution counters live on the query group and are
// read back through `engine.group()`, mirroring the spike's `ExecCounts`.

use {
    engine::{Engine, QueryGroup, Stored},
    std::cell::Cell,
};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Key {
    Text,
    Length,
    Doubled,
    Other,
    OtherLength,
}

#[derive(Default)]
struct Queries {
    length_runs: Cell<u64>,
    doubled_runs: Cell<u64>,
    other_length_runs: Cell<u64>,
}

impl QueryGroup for Queries {
    type Key = Key;

    fn execute(&self, engine: &Engine<Self>, key: &Key) -> Stored {
        match key {
            Key::Text | Key::Other => panic!("input queries are never executed"),
            Key::Length => {
                self.length_runs.set(self.length_runs.get() + 1);
                let text = engine.fetch::<String>(Key::Text);
                Stored::new(text.len())
            }
            Key::Doubled => {
                self.doubled_runs.set(self.doubled_runs.get() + 1);
                let length = engine.fetch::<usize>(Key::Length);
                Stored::new(*length * 2)
            }
            Key::OtherLength => {
                self.other_length_runs.set(self.other_length_runs.get() + 1);
                let other = engine.fetch::<String>(Key::Other);
                Stored::new(other.len())
            }
        }
    }
}

fn fresh() -> Engine<Queries> {
    Engine::new(Queries::default())
}

// Memoization: a body runs once, then the result is served from cache.
#[test]
fn body_runs_once_then_is_cached() {
    let mut engine = fresh();
    engine.set_input(Key::Text, "hello".to_owned());

    assert_eq!(*engine.fetch::<usize>(Key::Doubled), 10);
    assert_eq!(engine.group().length_runs.get(), 1);
    assert_eq!(engine.group().doubled_runs.get(), 1);

    // Re-fetch with no input change: all green, zero re-execution.
    assert_eq!(*engine.fetch::<usize>(Key::Doubled), 10);
    assert_eq!(engine.group().length_runs.get(), 1, "length stays cached");
    assert_eq!(engine.group().doubled_runs.get(), 1, "doubled stays cached");
}

// Automatic invalidation: changing an input recomputes its dependents (and only when a body actually
// reads the value, the new result flows through).
#[test]
fn changing_an_input_recomputes_dependents() {
    let mut engine = fresh();
    engine.set_input(Key::Text, "hello".to_owned());
    assert_eq!(*engine.fetch::<usize>(Key::Doubled), 10);

    engine.set_input(Key::Text, "worldwide".to_owned()); // len 9
    assert_eq!(*engine.fetch::<usize>(Key::Doubled), 18);
    assert_eq!(engine.group().length_runs.get(), 2, "length re-ran");
    assert_eq!(engine.group().doubled_runs.get(), 2, "doubled re-ran");
}

// Early cutoff #1 (input-level backdating): re-setting an input to an equal value recomputes nothing.
#[test]
fn no_op_input_set_recomputes_nothing() {
    let mut engine = fresh();
    engine.set_input(Key::Text, "hello".to_owned());
    let _ = engine.fetch::<usize>(Key::Doubled);

    engine.set_input(Key::Text, "hello".to_owned()); // identical value -> backdated
    let _ = engine.fetch::<usize>(Key::Doubled);
    assert_eq!(engine.group().length_runs.get(), 1, "input backdate cuts off before length");
    assert_eq!(engine.group().doubled_runs.get(), 1, "input backdate cuts off before doubled");
}

// Early cutoff #2 (value-eq within the chain): a changed input whose derived value is unchanged stops
// propagating. "hello" -> "world" changes Text, so Length re-runs, but both have length 5, so the
// unchanged Length value cuts off before Doubled.
#[test]
fn value_eq_cuts_off_mid_chain() {
    let mut engine = fresh();
    engine.set_input(Key::Text, "hello".to_owned());
    let _ = engine.fetch::<usize>(Key::Doubled);

    engine.set_input(Key::Text, "world".to_owned()); // different text, same length 5
    let _ = engine.fetch::<usize>(Key::Doubled);
    assert_eq!(engine.group().length_runs.get(), 2, "text changed -> length re-runs");
    assert_eq!(
        engine.group().doubled_runs.get(),
        1,
        "unchanged length value cuts off before doubled"
    );
}

// Dependency recording / isolation: the two chains share no input, so editing Text must not recompute
// OtherLength. No dependency was declared anywhere — the engine learned the edges by recording reads.
#[test]
fn independent_chains_do_not_cross_invalidate() {
    let mut engine = fresh();
    engine.set_input(Key::Text, "hello".to_owned());
    engine.set_input(Key::Other, "abc".to_owned());
    let _ = engine.fetch::<usize>(Key::Doubled);
    let _ = engine.fetch::<usize>(Key::OtherLength);
    assert_eq!(engine.group().other_length_runs.get(), 1);

    engine.set_input(Key::Text, "hello there".to_owned()); // edit only the Text chain
    let _ = engine.fetch::<usize>(Key::Doubled);
    let _ = engine.fetch::<usize>(Key::OtherLength);

    assert_eq!(engine.group().length_runs.get(), 2, "edited chain re-ran");
    assert_eq!(
        engine.group().other_length_runs.get(),
        1,
        "independent chain did not re-run"
    );
}

// --- Accidental-cycle guard (B1) -----------------------------------------------------------------
//
// The core assumes an acyclic recorded dependency graph. The one intended cycle (the package-interface
// fixed-point) is resolved inside a single body and never re-enters `fetch` on its own key. Any *other*
// cycle is an accidental host bug — exactly the kind R1 introduces when wiring real query bodies — and
// must fail loudly instead of recursing into a stack overflow.
mod cycle {
    use {
        engine::{Engine, QueryGroup, Stored},
        std::panic::{AssertUnwindSafe, catch_unwind},
    };

    #[derive(Clone, PartialEq, Eq, Hash, Debug)]
    enum Key {
        Ping,
        Pong,
    }

    struct Queries;

    impl QueryGroup for Queries {
        type Key = Key;

        // Ping fetches Pong fetches Ping: a two-node cycle with no input at the bottom.
        fn execute(&self, engine: &Engine<Self>, key: &Key) -> Stored {
            match key {
                Key::Ping => Stored::new(*engine.fetch::<usize>(Key::Pong)),
                Key::Pong => Stored::new(*engine.fetch::<usize>(Key::Ping)),
            }
        }
    }

    #[test]
    fn cyclic_query_group_panics_with_clear_message() {
        let engine = Engine::new(Queries);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = engine.fetch::<usize>(Key::Ping);
        }));
        let panic = result.expect_err("a cyclic query group must panic, not recurse forever");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(
            message.contains("query cycle detected") && message.contains("Ping"),
            "panic message should name the cycle, got: {message:?}"
        );
    }
}

// --- Fan-in (B2) ---------------------------------------------------------------------------------
//
// A derived node that reads MANY inputs — the shape of `package_naming` folding every file. Exercises the
// `.max()`-over-dependencies validation path: changing one of the many inputs recomputes the fold;
// changing none cuts it off.
mod fan_in {
    use {
        engine::{Engine, QueryGroup, Stored},
        std::cell::Cell,
    };

    #[derive(Clone, PartialEq, Eq, Hash, Debug)]
    enum Key {
        Input(u8),
        Sum,
    }

    const INPUTS: [u8; 4] = [0, 1, 2, 3];

    #[derive(Default)]
    struct Queries {
        sum_runs: Cell<u64>,
    }

    impl QueryGroup for Queries {
        type Key = Key;

        fn execute(&self, engine: &Engine<Self>, key: &Key) -> Stored {
            match key {
                Key::Input(_) => panic!("input queries are never executed"),
                Key::Sum => {
                    self.sum_runs.set(self.sum_runs.get() + 1);
                    let total: u32 = INPUTS
                        .iter()
                        .map(|index| *engine.fetch::<u32>(Key::Input(*index)))
                        .sum();
                    Stored::new(total)
                }
            }
        }
    }

    #[test]
    fn fan_in_recomputes_on_one_change_and_cuts_off_otherwise() {
        let mut engine = Engine::new(Queries::default());
        for index in INPUTS {
            engine.set_input(Key::Input(index), u32::from(index) + 1);
        }
        assert_eq!(*engine.fetch::<u32>(Key::Sum), 1 + 2 + 3 + 4);
        assert_eq!(engine.group().sum_runs.get(), 1);

        // Re-fetch with no change: the fold validates its four deps, none changed, no body run.
        let _ = engine.fetch::<u32>(Key::Sum);
        assert_eq!(engine.group().sum_runs.get(), 1, "no input changed -> fold cut off");

        // Change exactly one of the four inputs: the fold recomputes once.
        engine.set_input(Key::Input(2), 30u32);
        assert_eq!(*engine.fetch::<u32>(Key::Sum), 1 + 2 + 30 + 4);
        assert_eq!(engine.group().sum_runs.get(), 2, "one input changed -> fold re-ran once");
    }
}

// --- Diamond (B2) --------------------------------------------------------------------------------
//
// Base ← Root; B ← Base; C ← Base; D ← {B, C}. Base is a shared *derived* node reached via two paths.
// Exercises the `verified_at == revision` short-circuit: validating D walks Base through both arms but
// Base is validated/recomputed only once per revision, and a change to Root invalidates D through both
// arms while recomputing each node exactly once.
mod diamond {
    use {
        engine::{Engine, QueryGroup, Stored},
        std::cell::Cell,
    };

    #[derive(Clone, PartialEq, Eq, Hash, Debug)]
    enum Key {
        Root,
        Base,
        Left,
        Right,
        Down,
    }

    #[derive(Default)]
    struct Queries {
        base_runs: Cell<u64>,
        left_runs: Cell<u64>,
        right_runs: Cell<u64>,
        down_runs: Cell<u64>,
    }

    impl QueryGroup for Queries {
        type Key = Key;

        fn execute(&self, engine: &Engine<Self>, key: &Key) -> Stored {
            match key {
                Key::Root => panic!("input queries are never executed"),
                Key::Base => {
                    self.base_runs.set(self.base_runs.get() + 1);
                    Stored::new(*engine.fetch::<u32>(Key::Root) + 1)
                }
                Key::Left => {
                    self.left_runs.set(self.left_runs.get() + 1);
                    Stored::new(*engine.fetch::<u32>(Key::Base) * 2)
                }
                Key::Right => {
                    self.right_runs.set(self.right_runs.get() + 1);
                    Stored::new(*engine.fetch::<u32>(Key::Base) * 3)
                }
                Key::Down => {
                    self.down_runs.set(self.down_runs.get() + 1);
                    Stored::new(*engine.fetch::<u32>(Key::Left) + *engine.fetch::<u32>(Key::Right))
                }
            }
        }
    }

    #[test]
    fn shared_node_validated_once_and_invalidates_through_both_arms_once() {
        let mut engine = Engine::new(Queries::default());
        engine.set_input(Key::Root, 10u32); // Base=11, Left=22, Right=33, Down=55
        assert_eq!(*engine.fetch::<u32>(Key::Down), 55);
        assert_eq!(engine.group().base_runs.get(), 1, "Base computed once despite two paths");
        assert_eq!(engine.group().left_runs.get(), 1);
        assert_eq!(engine.group().right_runs.get(), 1);
        assert_eq!(engine.group().down_runs.get(), 1);

        engine.set_input(Key::Root, 20u32); // Base=21, Left=42, Right=63, Down=105
        assert_eq!(*engine.fetch::<u32>(Key::Down), 105);
        assert_eq!(engine.group().base_runs.get(), 2, "Base recomputed exactly once, not per arm");
        assert_eq!(engine.group().left_runs.get(), 2);
        assert_eq!(engine.group().right_runs.get(), 2);
        assert_eq!(engine.group().down_runs.get(), 2, "Down recomputed once through both arms");
    }
}

// --- Partial-change cutoff (B2) ------------------------------------------------------------------
//
// Fan-in `Fold` reads two inputs P and Q; downstream `Tens` reads `Fold`. One input is re-set to an equal
// value (input-level backdate) while the other changes to a different value. `Fold` recomputes because Q
// really changed, but its own output (derived only from P) is unchanged, so value-eq cutoff propagation
// keeps `Tens` green.
mod partial_change {
    use {
        engine::{Engine, QueryGroup, Stored},
        std::cell::Cell,
    };

    #[derive(Clone, PartialEq, Eq, Hash, Debug)]
    enum Key {
        P,
        Q,
        Fold,
        Tens,
    }

    #[derive(Default)]
    struct Queries {
        fold_runs: Cell<u64>,
        tens_runs: Cell<u64>,
    }

    impl QueryGroup for Queries {
        type Key = Key;

        fn execute(&self, engine: &Engine<Self>, key: &Key) -> Stored {
            match key {
                Key::P | Key::Q => panic!("input queries are never executed"),
                Key::Fold => {
                    self.fold_runs.set(self.fold_runs.get() + 1);
                    // Reads both inputs (records both deps) but the output depends only on P, so a change
                    // confined to Q recomputes the fold without changing its value.
                    let p = engine.fetch::<String>(Key::P);
                    let _q = engine.fetch::<String>(Key::Q);
                    Stored::new(p.len())
                }
                Key::Tens => {
                    self.tens_runs.set(self.tens_runs.get() + 1);
                    Stored::new(*engine.fetch::<usize>(Key::Fold) * 10)
                }
            }
        }
    }

    #[test]
    fn fold_recomputes_but_unchanged_value_cuts_off_downstream() {
        let mut engine = Engine::new(Queries::default());
        engine.set_input(Key::P, "ab".to_owned());
        engine.set_input(Key::Q, "x".to_owned());
        assert_eq!(*engine.fetch::<usize>(Key::Tens), 20);
        assert_eq!(engine.group().fold_runs.get(), 1);
        assert_eq!(engine.group().tens_runs.get(), 1);

        engine.set_input(Key::P, "ab".to_owned()); // identical -> backdated, no change
        engine.set_input(Key::Q, "yyyy".to_owned()); // real change
        assert_eq!(*engine.fetch::<usize>(Key::Tens), 20);
        assert_eq!(engine.group().fold_runs.get(), 2, "Q changed -> fold re-ran");
        assert_eq!(
            engine.group().tens_runs.get(),
            1,
            "fold value unchanged -> downstream cut off"
        );
    }
}

// --- Input removal (A2 core support) -------------------------------------------------------------
//
// Models the package-file set as an input list plus per-member inputs, the shape A2 prescribes: a member
// is removed by re-setting the membership list (without it) and `remove_input` on its slot. The fold
// observes the member gone without ever fetching the removed input, and without a separate mirrored set.
mod removal {
    use {
        engine::{Engine, QueryGroup, Stored},
        std::cell::Cell,
    };

    #[derive(Clone, PartialEq, Eq, Hash, Debug)]
    enum Key {
        Members,
        Member(u8),
        Total,
    }

    #[derive(Default)]
    struct Queries {
        total_runs: Cell<u64>,
    }

    impl QueryGroup for Queries {
        type Key = Key;

        fn execute(&self, engine: &Engine<Self>, key: &Key) -> Stored {
            match key {
                Key::Members | Key::Member(_) => panic!("input queries are never executed"),
                Key::Total => {
                    self.total_runs.set(self.total_runs.get() + 1);
                    let members = engine.fetch::<Vec<u8>>(Key::Members);
                    let total: u32 = members
                        .iter()
                        .map(|id| *engine.fetch::<u32>(Key::Member(*id)))
                        .sum();
                    Stored::new(total)
                }
            }
        }
    }

    #[test]
    fn removing_an_input_drops_it_from_a_set_fold() {
        let mut engine = Engine::new(Queries::default());
        engine.set_input(Key::Members, vec![0u8, 1u8]);
        engine.set_input(Key::Member(0), 10u32);
        engine.set_input(Key::Member(1), 20u32);
        assert_eq!(*engine.fetch::<u32>(Key::Total), 30);
        assert_eq!(engine.group().total_runs.get(), 1);
        let slots_before = engine.slot_count();

        // Delete member 1: shrink the membership input and remove the member's own input slot.
        engine.set_input(Key::Members, vec![0u8]);
        engine.remove_input(&Key::Member(1));
        assert_eq!(*engine.fetch::<u32>(Key::Total), 10, "removed member dropped from the fold");
        assert_eq!(engine.group().total_runs.get(), 2, "fold recomputed over the smaller set");
        assert_eq!(
            engine.slot_count(),
            slots_before,
            "removal leaves a tombstone in place, not a second mirrored slot"
        );
    }

    #[test]
    fn removed_input_can_be_re_added() {
        let mut engine = Engine::new(Queries::default());
        engine.set_input(Key::Members, vec![0u8]);
        engine.set_input(Key::Member(0), 5u32);
        assert_eq!(*engine.fetch::<u32>(Key::Total), 5);

        engine.set_input(Key::Members, Vec::<u8>::new());
        engine.remove_input(&Key::Member(0));
        assert_eq!(*engine.fetch::<u32>(Key::Total), 0, "empty set folds to zero");

        // Re-add the same member: add→delete→re-add must converge, not read a stale removed slot.
        engine.set_input(Key::Members, vec![0u8]);
        engine.set_input(Key::Member(0), 7u32);
        assert_eq!(*engine.fetch::<u32>(Key::Total), 7, "re-added member folds with its new value");
    }
}
