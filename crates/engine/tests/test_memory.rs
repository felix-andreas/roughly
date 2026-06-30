//! Resident-memory benchmark: the new engine vs. the production `analysis` crate at 9k / 94k / 281k LoC
//! (cutover done-bar (e): "memory bounded at 281k"). Measures *live heap bytes* — net `alloc - dealloc`
//! held by each structure once fully built and warmed — via a counting global allocator, which is a clean,
//! deterministic proxy for resident set size of the data structures (it excludes transient scratch that has
//! already been freed, exactly the steady-state figure we care about).
//!
//! The engine is built and warmed (every file's `Diagnostics` fetched, so every memo is live), then its
//! live bytes are read and it is dropped; the `analysis` path is then built with `run_full` and measured the
//! same way. Each size is measured independently from a drained baseline so the two figures are comparable.
//!
//! Heavy (`#[ignore]`); run manually:
//! `cargo test -p engine --release --test test_memory -- --ignored --nocapture --test-threads=1`.

use {
    analysis::{Analysis, CheckConfig, LintConfig, naming::DocumentKind, run_full},
    engine::{
        Engine,
        queries::{Config, FileDiagnostics, FileId, Key, RoughlyQueries},
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

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as i64, Ordering::Relaxed);
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
    if index % CHAIN_LEN != 0 {
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
            typing: true,
            strict: false,
            unused: false,
            lint: LintConfig::default(),
        },
    );
    for index in 0..file_count {
        engine.set_input(Key::SourceText(index as FileId), generate_source(index));
        engine.set_input(Key::DocumentKind(index as FileId), DocumentKind::Package);
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
