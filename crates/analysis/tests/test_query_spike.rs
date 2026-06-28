// Cross-check (correctness) and measurement (overhead) harness for the `query_spike` red-green memo
// engine. Entirely gated behind the `query-spike` feature, so without it this binary is empty.
//
// Run with:  cargo test -p analysis --features query-spike
// Bench:     cargo test -p analysis --release --features query-spike -- --ignored --nocapture
#![cfg(feature = "query-spike")]

use {
    analysis::query_spike::{Engine, ExecCounts, FileId},
    std::time::Instant,
};

// A deterministic xorshift64 generator (same shape as the seeded-soak generator in test_incremental.rs)
// so the whole cross-check is reproducible without a dependency.
struct Xorshift(u64);

impl Xorshift {
    fn next_u64(&mut self) -> u64 {
        let mut state = self.0;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        self.0 = state;
        state
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

fn slot_path(index: usize, is_script: bool) -> String {
    if is_script {
        format!("/pkg/scripts/f{index}.R")
    } else {
        format!("/pkg/R/f{index}.R")
    }
}

// A short package source over a small name/type pool, mirroring the soak generator: plain assigns,
// bare-block and conditional globals, cross-file references, `@type`/`@alias` definitions, type-annotated
// values, and typed-function interfaces. Exercises the constructs lower + local-naming actually branch on.
fn random_source(rng: &mut Xorshift) -> String {
    let names = ["a", "b", "c", "d"];
    let types = ["T1", "T2", "T3"];
    let line_count = 1 + rng.below(3);
    let mut lines = Vec::new();
    for _ in 0..line_count {
        let name = names[rng.below(names.len())];
        let other = names[rng.below(names.len())];
        let type_name = types[rng.below(types.len())];
        lines.push(match rng.below(9) {
            0 => format!("{name} <- 1L"),
            1 => format!("{name} <- \"x\""),
            2 => format!("{{\n  {name} <- 2L\n}}"),
            3 => format!("if (TRUE) {{\n  {name} <- 3L\n}}"),
            4 => format!("{name} <- {other}"),
            5 => format!("#: @type {type_name} {{integer}}"),
            6 => format!("#: @alias {type_name} {{double}}"),
            7 => format!("#: {type_name}\n{name} <- 1L"),
            _ => format!("#: fn(value: integer) -> integer\n{name} <- function(value) value"),
        });
    }
    lines.push(String::new());
    lines.join("\n")
}

// Fetch each live file's whole chain through the engine and assert the memoized result equals a fresh
// from-scratch recompute (the drift oracle: framework analog of incremental == full rebuild).
fn assert_memo_matches_recompute(engine: &Engine, live: &[Option<LiveDocument>]) {
    for (index, slot) in live.iter().enumerate() {
        let Some(document) = slot else { continue };
        let file = index as FileId;
        let memo_sexp = engine.parse_sexp(file);
        let memo_lower = engine.fetch_lower(file);
        let memo_naming = engine.fetch_local_naming(file);

        let (fresh_sexp, fresh_lower, fresh_naming) = engine.recompute_chain(file);
        assert_eq!(memo_sexp, fresh_sexp, "parse drift at {}", document.path());
        assert_eq!(*memo_lower, fresh_lower, "lower drift at {}", document.path());
        assert_eq!(
            *memo_naming,
            fresh_naming,
            "local_naming drift at {}",
            document.path()
        );
    }
}

struct LiveDocument {
    index: usize,
    is_script: bool,
    source: String,
}

impl LiveDocument {
    fn path(&self) -> String {
        slot_path(self.index, self.is_script)
    }
}

// Drive a deterministic stream of add / edit / delete / package<->script moves over a small slot pool,
// asserting after every operation that every live file's memoized chain equals a fresh recompute. This is
// the engine's automatic-invalidation correctness proof.
#[test]
fn cross_check_incremental_matches_full_rebuild() {
    const OPERATION_COUNT: usize = 2000;
    const SLOT_COUNT: usize = 6;

    let mut engine = Engine::new();
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    let mut live: Vec<Option<LiveDocument>> = (0..SLOT_COUNT).map(|_| None).collect();

    for _ in 0..OPERATION_COUNT {
        let index = rng.below(SLOT_COUNT);
        let file = index as FileId;
        match (&live[index], rng.below(10)) {
            // Delete a live document.
            (Some(_), 0) => {
                engine.remove_input(file);
                live[index] = None;
            }
            // package <-> script move: the document's source stays, its path-kind flips, so the naming
            // body's tracked kind read must re-run it even when text is unchanged.
            (Some(document), 1) => {
                let is_script = !document.is_script;
                let source = document.source.clone();
                engine.set_input(file, &slot_path(index, is_script), source.clone());
                live[index] = Some(LiveDocument {
                    index,
                    is_script,
                    source,
                });
            }
            // Whole-document edit.
            (Some(document), _) => {
                let is_script = document.is_script;
                let source = random_source(&mut rng);
                engine.set_input(file, &slot_path(index, is_script), source.clone());
                live[index] = Some(LiveDocument {
                    index,
                    is_script,
                    source,
                });
            }
            // Add at the slot's current path kind.
            (None, _) => {
                let source = random_source(&mut rng);
                engine.set_input(file, &slot_path(index, false), source.clone());
                live[index] = Some(LiveDocument {
                    index,
                    is_script: false,
                    source,
                });
            }
        }

        assert_memo_matches_recompute(&engine, &live);
    }
}

// Early-cutoff proof #1: editing file A must not re-execute file B's chain bodies (B does not depend on
// A's input). Proven with the per-query body-execution counters.
#[test]
fn editing_one_file_does_not_recompute_another() {
    let mut engine = Engine::new();
    engine.set_input(0, "/pkg/R/a.R", "a <- 1L\n".to_owned());
    engine.set_input(1, "/pkg/R/b.R", "b <- 2L\n".to_owned());
    // Warm both chains.
    let _ = engine.fetch_local_naming(0);
    let _ = engine.fetch_local_naming(1);

    let before = engine.exec_counts();
    // Edit only file A.
    engine.set_input(0, "/pkg/R/a.R", "a <- 99L\n".to_owned());
    let _ = engine.fetch_local_naming(0);
    let _ = engine.fetch_local_naming(1);
    let after = engine.exec_counts();

    // File A's chain re-ran exactly once per phase; file B's bodies did not run at all.
    assert_eq!(after.parse - before.parse, 1, "only file A reparsed");
    assert_eq!(after.lower - before.lower, 1, "only file A re-lowered");
    assert_eq!(after.naming - before.naming, 1, "only file A re-named");
}

// Early-cutoff proof #2: value-eq cutoff, both at the input layer and within the chain.
// - A no-op edit (identical text) backdates the input, so nothing re-runs (input-level cutoff).
// - A trailing-comment edit changes the parse tree but lowers to the same Module, cutting off before
//   local_naming (the body-counter shows lower re-runs, naming does not).
#[test]
fn value_eq_cuts_off_within_the_chain() {
    let mut engine = Engine::new();
    let source = "#: integer\nx <- 1L\n";
    engine.set_input(0, "/pkg/R/a.R", source.to_owned());
    let _ = engine.fetch_local_naming(0);

    // No-op edit: identical (text, kind) backdates the input, so the whole chain stays green.
    let before = engine.exec_counts();
    engine.set_input(0, "/pkg/R/a.R", source.to_owned());
    let _ = engine.fetch_local_naming(0);
    let after = engine.exec_counts();
    assert_eq!(after.parse - before.parse, 0, "no-op edit backdates input");
    assert_eq!(
        after.lower - before.lower,
        0,
        "input backdate cuts off before lower"
    );
    assert_eq!(
        after.naming - before.naming,
        0,
        "input backdate cuts off before naming"
    );

    // Trailing-comment edit: changes the source/tree but the Module is identical (the comment is not
    // lowered and existing token byte ranges are unchanged), so lower re-runs but naming cuts off.
    let before = engine.exec_counts();
    engine.set_input(0, "/pkg/R/a.R", format!("{source}# trailing\n"));
    let _ = engine.fetch_local_naming(0);
    let after = engine.exec_counts();
    assert_eq!(after.parse - before.parse, 1, "comment edit re-runs parse");
    assert_eq!(after.lower - before.lower, 1, "comment edit re-runs lower");
    assert_eq!(
        after.naming - before.naming,
        0,
        "identical Module cuts off before local_naming"
    );
}

// Warm no-op recheck: re-fetching with no input change is all-green (zero body executions).
#[test]
fn warm_recheck_is_all_green() {
    let mut engine = Engine::new();
    for index in 0..50u32 {
        engine.set_input(index, &format!("/pkg/R/f{index}.R"), "a <- 1L\n".to_owned());
    }
    for index in 0..50u32 {
        let _ = engine.fetch_local_naming(index);
    }
    let before = engine.exec_counts();
    for index in 0..50u32 {
        let _ = engine.fetch_local_naming(index);
    }
    let after = engine.exec_counts();
    assert_eq!(after.parse, before.parse, "warm recheck runs no parse body");
    assert_eq!(after.lower, before.lower, "warm recheck runs no lower body");
    assert_eq!(after.naming, before.naming, "warm recheck runs no naming body");
}

// Reproducible overhead measurements: cold build, warm no-op recheck, single-file per-edit re-fetch
// (incremental) vs a direct from-scratch recompute of that one chain, plus memo-table overhead, across a
// few package sizes.
#[test]
#[ignore = "timing benchmark, run manually"]
fn benchmark_query_spike() {
    for &file_count in &[200usize, 1000, 5000] {
        run_measurement(file_count);
    }
}

fn run_measurement(file_count: usize) {
    let mut rng = Xorshift(0xDEAD_BEEF_1234_5678);
    let sources: Vec<String> = (0..file_count).map(|_| random_source(&mut rng)).collect();

    let mut engine = Engine::new();
    for (index, source) in sources.iter().enumerate() {
        engine.set_input(index as FileId, &format!("/pkg/R/f{index}.R"), source.clone());
    }

    let cold_start = Instant::now();
    for index in 0..file_count as FileId {
        let _ = engine.fetch_local_naming(index);
    }
    let cold = cold_start.elapsed();

    let warm_start = Instant::now();
    for index in 0..file_count as FileId {
        let _ = engine.fetch_local_naming(index);
    }
    let warm = warm_start.elapsed();

    // Single-file edit -> re-fetch that file's chain through the engine (incremental path).
    let edited = (file_count / 2) as FileId;
    let new_source = random_source(&mut rng);
    let edit_start = Instant::now();
    engine.set_input(edited, &format!("/pkg/R/f{edited}.R"), new_source.clone());
    let _ = engine.fetch_local_naming(edited);
    let per_edit = edit_start.elapsed();

    // Direct from-scratch recompute of that one chain (the non-memo baseline body cost).
    let direct_start = Instant::now();
    let _ = engine.recompute_chain(edited);
    let direct = direct_start.elapsed();

    let entries = engine.memo_entry_count();
    let overhead_bytes = engine.estimated_memo_overhead_bytes();

    println!("query-spike measurement: {file_count} files");
    println!("  cold full build:        {cold:?}");
    println!("  warm no-op recheck:     {warm:?}");
    println!("  single-file per-edit:   {per_edit:?}");
    println!("  direct chain recompute: {direct:?}");
    println!("  memo entries:           {entries} (3 x {file_count})");
    println!(
        "  memo bookkeeping bytes: {overhead_bytes} (~{} B/file; headers+dep-edges, excl. payloads)",
        overhead_bytes / file_count.max(1)
    );
}

fn _assert_exec_counts_is_copy(counts: ExecCounts) -> ExecCounts {
    counts
}
