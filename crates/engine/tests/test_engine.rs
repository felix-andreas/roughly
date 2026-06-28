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

#[derive(Clone, PartialEq, Eq, Hash)]
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
