//! Memory behavior of the engine. Two parts:
//!
//! 1. A fast, always-on **memo-table growth bound**: derived keys are minted per file *and per
//!    symbol*, so an edit stream that keeps minting fresh global names (typing a name character by
//!    character mints one per keystroke) grows the memo table without bound;
//!    `Engine::evict_stale_memos` is the bound, and the test proves eviction returns the table to the
//!    live working set while answers stay correct.
//!
//! 2. A resident-memory benchmark: the engine vs. the from-scratch `analysis` crate at 9k / 94k /
//!    281k LoC. Measures *live heap bytes* — net `alloc - dealloc` held by each structure once fully
//!    built and warmed — via a counting global allocator, which is a clean, deterministic proxy for
//!    resident set size of the data structures (it excludes transient scratch that has already been
//!    freed, exactly the steady-state figure we care about). The engine is built and warmed (every
//!    file's `Diagnostics` fetched, so every memo is live), then its live bytes are read and it is
//!    dropped; the `analysis` path is then built with `run_full` and measured the same way. Each size
//!    is measured independently from a drained baseline so the two figures are comparable.
//!    Heavy (`#[ignore]`); run manually:
//!    `cargo test -p engine --release --test test_memory -- --ignored --nocapture --test-threads=1`.

use {
    analysis::{Analysis, CheckConfig, LintConfig, naming::DocumentKind, run_full},
    engine::{
        Engine,
        queries::{Config, FileDiagnostics, FileId, Key, RoughlyQueries, parse_source_input},
    },
    std::{
        alloc::{GlobalAlloc, Layout, System},
        path::PathBuf,
        sync::atomic::{AtomicI64, Ordering},
    },
};

// ----------------------------------------------------------------------------------------------------
// Counting allocator: live heap bytes = net of every alloc/dealloc routed through the system allocator.
// `realloc`/`alloc_zeroed` use the `GlobalAlloc` default bodies, which call these two, so they are counted.
// ----------------------------------------------------------------------------------------------------

struct CountingAllocator;

static LIVE_BYTES: AtomicI64 = AtomicI64::new(0);
// Cumulative churn: every allocation ever made (count and bytes), never decremented. The *delta*
// across an operation is its allocation churn — the measure the per-edit churn bench reports.
static TOTAL_ALLOCATIONS: AtomicI64 = AtomicI64::new(0);
static TOTAL_ALLOCATED_BYTES: AtomicI64 = AtomicI64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as i64, Ordering::Relaxed);
            TOTAL_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            TOTAL_ALLOCATED_BYTES.fetch_add(layout.size() as i64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE_BYTES.fetch_sub(layout.size() as i64, Ordering::Relaxed);
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn live_bytes() -> i64 {
    LIVE_BYTES.load(Ordering::Relaxed)
}

fn total_allocations() -> i64 {
    TOTAL_ALLOCATIONS.load(Ordering::Relaxed)
}

fn total_allocated_bytes() -> i64 {
    TOTAL_ALLOCATED_BYTES.load(Ordering::Relaxed)
}

// ----------------------------------------------------------------------------------------------------
// Synthetic cross-file package (the shape `test_benchmark.rs` uses: bounded-depth chains so the workspace
// is realistic — a few imports a few hops deep — rather than an N-deep spine).
// ----------------------------------------------------------------------------------------------------

const ITEMS_PER_FILE: usize = 5;
const CHAIN_LEN: usize = 8;

fn generate_source(index: usize) -> String {
    let mut source = String::new();
    for item in 0..ITEMS_PER_FILE {
        source.push_str(&format!("g_{index}_{item} <- function() 1L\n"));
    }
    if !index.is_multiple_of(CHAIN_LEN) {
        let previous = index - 1;
        for item in 0..ITEMS_PER_FILE {
            source.push_str(&format!("u_{index}_{item} <- g_{previous}_{item}()\n"));
        }
    }
    source
}

fn file_count_for_loc(target_loc: usize) -> usize {
    (target_loc / (2 * ITEMS_PER_FILE)).max(4)
}

fn total_loc(file_count: usize) -> usize {
    (0..file_count)
        .map(|index| generate_source(index).lines().count())
        .sum()
}

fn build_and_warm_new_engine(file_count: usize) -> Engine<RoughlyQueries> {
    let mut engine = Engine::new(RoughlyQueries::new());
    engine.set_input(
        Key::ProjectFiles,
        (0..file_count as FileId).collect::<Vec<_>>(),
    );
    engine.set_input(
        Key::Config,
        Config {
            check: CheckConfig {
                typing: true,
                strict: false,
                unused: false,
            },
            lint: LintConfig::default(),
        },
    );
    for index in 0..file_count {
        engine.set_input(
            Key::SourceText(index as FileId),
            parse_source_input(&generate_source(index)),
        );
        engine.set_input(Key::DocumentKind(index as FileId), DocumentKind::Package);
        engine.set_input(
            Key::FileName(index as FileId),
            PathBuf::from(format!("/pkg/R/file_{index:06}.R")),
        );
    }
    for index in 0..file_count as FileId {
        let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(index));
    }
    engine
}

fn build_old_engine(file_count: usize) -> Analysis {
    let mut analysis_state = Analysis::new(
        PathBuf::from("/pkg"),
        LintConfig::default(),
        CheckConfig {
            unused: false,
            typing: true,
            strict: false,
        },
    );
    for index in 0..file_count {
        analysis_state
            .add_document_from_source(
                PathBuf::from(format!("/pkg/R/file_{index:06}.R")),
                &generate_source(index),
            )
            .expect("benchmark source should parse");
    }
    run_full(&mut analysis_state);
    analysis_state
}

// ----------------------------------------------------------------------------------------------------
// Growth bound under an edit stream (fast, slot counts, always on).
// ----------------------------------------------------------------------------------------------------

#[test]
fn edit_stream_growth_is_bounded_by_eviction() {
    let mut engine = Engine::new(RoughlyQueries::new());
    engine.set_input(Key::ProjectFiles, vec![0 as FileId]);
    engine.set_input(
        Key::Config,
        Config {
            check: CheckConfig {
                typing: true,
                strict: false,
                unused: false,
            },
            lint: LintConfig::default(),
        },
    );
    engine.set_input(Key::DocumentKind(0), DocumentKind::Package);
    engine.set_input(Key::FileName(0), PathBuf::from("/pkg/R/file_0.R"));
    engine.set_input(Key::SourceText(0), parse_source_input("value <- start()\n"));
    let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(0));
    let baseline = engine.slot_count();

    // The pathological (and realistic) stream: every edit references a fresh global name, exactly
    // what typing an identifier does one keystroke at a time. Each name mints per-symbol derived
    // keys that no later revision ever fetches again.
    for index in 0..300u32 {
        engine.set_input(
            Key::SourceText(0),
            parse_source_input(&format!("value <- name_{index}()\n")),
        );
        let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(0));
    }
    let grown = engine.slot_count();
    assert!(
        grown > baseline + 250,
        "per-symbol keys accrete across the stream (else this test guards nothing): {baseline} -> {grown}"
    );

    // Evict everything not read in the last few revisions: the table returns to the live working
    // set (the inputs plus the chain the final fetch validated).
    let dropped = engine.evict_stale_memos(4);
    let bounded = engine.slot_count();
    assert!(dropped > 0, "the stream left stale memos to drop");
    assert!(
        bounded <= baseline + 16,
        "eviction bounds the table near the live working set: {bounded} vs baseline {baseline}"
    );

    // Correctness after eviction, part 1: the surviving chain still answers.
    let after_eviction = engine.fetch::<FileDiagnostics>(Key::Diagnostics(0));

    // Part 2: an edit that references an *evicted* symbol again forces the dropped per-symbol keys
    // to recompute through a live dependent — the answer must equal a fresh engine's for the same
    // final state, not crash and not go stale.
    engine.set_input(
        Key::SourceText(0),
        parse_source_input("value <- name_5()\n"),
    );
    let revalidated = engine.fetch::<FileDiagnostics>(Key::Diagnostics(0));

    let mut fresh = Engine::new(RoughlyQueries::new());
    fresh.set_input(Key::ProjectFiles, vec![0 as FileId]);
    fresh.set_input(
        Key::Config,
        Config {
            check: CheckConfig {
                typing: true,
                strict: false,
                unused: false,
            },
            lint: LintConfig::default(),
        },
    );
    fresh.set_input(Key::DocumentKind(0), DocumentKind::Package);
    fresh.set_input(Key::FileName(0), PathBuf::from("/pkg/R/file_0.R"));
    fresh.set_input(
        Key::SourceText(0),
        parse_source_input("value <- name_5()\n"),
    );
    let fresh_result = fresh.fetch::<FileDiagnostics>(Key::Diagnostics(0));
    // Raw type errors carry interner Symbols, whose ids depend on each engine's interning history;
    // every rendered (string-bearing) field must match exactly, and the raw ones by shape.
    assert_eq!(
        revalidated.naming, fresh_result.naming,
        "an evicted-then-revalidated chain answers exactly like a fresh engine"
    );
    assert_eq!(revalidated.package_naming, fresh_result.package_naming);
    assert!(
        revalidated
            .package_naming
            .iter()
            .any(|diagnostic| diagnostic.message.contains("name_5")),
        "the re-referenced evicted symbol resolves to the same unresolved-name diagnostic: {:?}",
        revalidated.package_naming
    );
    assert_eq!(revalidated.lowering, fresh_result.lowering);
    assert_eq!(revalidated.lint, fresh_result.lint);
    assert_eq!(revalidated.unused, fresh_result.unused);
    assert_eq!(
        revalidated.type_errors.len(),
        fresh_result.type_errors.len()
    );
    drop(after_eviction);
}

#[test]
#[ignore = "memory benchmark; run manually with --release --nocapture --test-threads=1"]
fn memory_new_vs_old_resident() {
    let sizes: Vec<usize> = match std::env::var("MEM_LOC") {
        Ok(value) => value
            .split(',')
            .filter_map(|entry| entry.trim().parse().ok())
            .collect(),
        Err(_) => vec![9_375usize, 93_750, 281_250],
    };

    println!();
    println!(
        "  {:>8}  {:>7}  {:>14}  {:>14}  {:>7}",
        "LoC", "files", "new resident", "old resident", "ratio"
    );
    for target_loc in sizes {
        let file_count = file_count_for_loc(target_loc);
        let loc = total_loc(file_count);

        let baseline = live_bytes();
        let engine = build_and_warm_new_engine(file_count);
        let new_bytes = (live_bytes() - baseline).max(0) as f64;
        drop(engine);

        let baseline = live_bytes();
        let analysis_state = build_old_engine(file_count);
        let old_bytes = (live_bytes() - baseline).max(0) as f64;
        drop(analysis_state);

        let new_mb = new_bytes / (1024.0 * 1024.0);
        let old_mb = old_bytes / (1024.0 * 1024.0);
        let ratio = if new_bytes > 0.0 {
            old_bytes / new_bytes
        } else {
            0.0
        };
        println!(
            "  {loc:>8}  {file_count:>7}  {new_mb:>11.1} MB  {old_mb:>11.1} MB  {ratio:>6.2}x"
        );
    }
    println!();
}

// ----------------------------------------------------------------------------------------------------
// Per-edit allocation churn (heavy; run manually). Reports allocations and allocated bytes per warm
// body-edit recheck — the churn the design bar says to minimize. Not asserted: raw counts move with
// rustc/std versions; the value is the printed trend when a change claims to reduce churn.
// ----------------------------------------------------------------------------------------------------

#[test]
#[ignore = "churn benchmark; run manually with --release --nocapture --test-threads=1"]
fn churn_per_body_edit() {
    const TARGET_LOC: usize = 10_000;
    const ROUNDS: usize = 20;
    let file_count = file_count_for_loc(TARGET_LOC);
    let mut engine = build_and_warm_new_engine(file_count);
    let edit_file = {
        let middle = file_count / 2;
        (middle - (middle % CHAIN_LEN) + CHAIN_LEN / 2) as FileId
    };
    let referrer = edit_file + 1;

    // One untimed warmup round per parity so both edit directions have warm memos.
    for round in 0..2 {
        engine.set_input(
            Key::SourceText(edit_file),
            parse_source_input(&generate_edited_source(edit_file as usize, round % 2 == 0)),
        );
        let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(edit_file));
        let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(referrer));
    }

    let allocations_before = total_allocations();
    let bytes_before = total_allocated_bytes();
    for round in 0..ROUNDS {
        engine.set_input(
            Key::SourceText(edit_file),
            parse_source_input(&generate_edited_source(edit_file as usize, round % 2 == 0)),
        );
        let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(edit_file));
        let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(referrer));
    }
    let allocations = (total_allocations() - allocations_before) / ROUNDS as i64;
    let bytes = (total_allocated_bytes() - bytes_before) / ROUNDS as i64;
    println!(
        "churn per body edit at {} LoC: {allocations} allocations, {} KiB allocated",
        total_loc(file_count),
        bytes / 1024,
    );
}

// The benchmark's toggling body edit: flips every function's return literal, changing each scheme
// so the referrer genuinely re-typechecks (the same shape `test_benchmark.rs` measures).
fn generate_edited_source(index: usize, returns_double: bool) -> String {
    let literal = if returns_double { "1.0" } else { "1L" };
    let mut source = String::new();
    for item in 0..ITEMS_PER_FILE {
        source.push_str(&format!("g_{index}_{item} <- function() {literal}\n"));
    }
    if !index.is_multiple_of(CHAIN_LEN) {
        let previous = index - 1;
        for item in 0..ITEMS_PER_FILE {
            source.push_str(&format!("u_{index}_{item} <- g_{previous}_{item}()\n"));
        }
    }
    source
}
