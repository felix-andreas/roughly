//! R3 — the O(blast-radius) per-edit benchmark (`DESIGN.md` §8 R3: "per-edit cost measured
//! O(blast-radius) and competitive with (better than) the hand-rolled path").
//!
//! # What this measures
//!
//! A single-file *body* edit and the time to re-derive the edited file's (and its referrer's) diagnostics,
//! at several package sizes, against the production `analysis` crate driven through the same edit.
//!
//! # What it shows (the real, measured result)
//!
//! Two distinct claims, only the first of which is size-independent:
//!
//! 1. **Recompute is blast-radius bounded — flat in N.** Proven by the exec counters in
//!    [`body_edit_recheck_is_blast_radius_bounded`] and reprinted by the table: a body edit performs
//!    **zero** O(package) recomputation — the names-only `PackageSymbolIndex` does not re-fold, and the
//!    declarations-only-cutoff `PackageTypeDefinitions` does not re-fold either — and exactly **two** files
//!    re-typecheck (the edited file and its referrer), independent of N. The expensive work, HM inference,
//!    is genuinely O(blast-radius).
//!
//! 2. **Wall time is ~10–13× lower than production at every size, but both scale ~linearly in N.** The
//!    engine is *not* flat in wall time: confirming the package-wide folds (`PackageSymbolIndex`,
//!    `PackageTypeDefinitions`, the package-naming candidate order) are unchanged is an O(package)
//!    *validation walk* — it touches each file's per-file memo once (a hash lookup + an early-cutoff bump,
//!    no clone after the `validate` lazy-dependency fix, no inference). This is the inherent cost of
//!    demand-driven red-green over an all-files fold (salsa pays it too); driving it sub-linear needs the
//!    durability / changed-input-tracking slice `DESIGN.md` §1 defers, or sharded per-module def-maps.
//!    Production's O(package) term, by contrast, is HM re-inference plus a per-round interface-table
//!    rebuild and a type-definition-fingerprint render over every module — far costlier per unit N, which
//!    is why the engine is an order of magnitude faster at every size even though both grow.
//!
//! Measured here (release, mid-chain body edit; your hardware will differ, but the *shape* is stable):
//!
//! ```text
//!     LoC    files   new per-edit   old per-edit   ratio   recompute  PSI/PTD refolds
//!    9 375    1 000       1.05 ms       13.8  ms   13.2x       2        0 / 0
//!   93 750   10 000      21.3  ms      233    ms   10.9x       2        0 / 0
//!  281 250   30 000      72.7  ms      771    ms   10.6x       2        0 / 0
//! ```
//!
//! # Layout
//!
//! - [`body_edit_recheck_is_blast_radius_bounded`] runs by default (small N): it is the exec-counter
//!   proof that the recompute set is blast-radius bounded and the index does not re-fold. This is the
//!   committed correctness witness for the perf claim.
//! - [`benchmark_new_vs_old_per_edit`] is `#[ignore]` (heavy): it prints the `size | new | old | ratio`
//!   table at 10k / 100k (/ 300k) LoC. Run it manually:
//!   `cargo test -p engine --release --test test_benchmark -- --ignored --nocapture`.

use {
    analysis::{Analysis, CheckConfig, LintConfig, naming::DocumentKind, run_full},
    engine::{
        Engine,
        queries::{Config, FileDiagnostics, FileId, Key, RoughlyQueries},
    },
    std::{
        path::PathBuf,
        time::{Duration, Instant},
    },
};

// ----------------------------------------------------------------------------------------------------
// Synthetic cross-file package generator
// ----------------------------------------------------------------------------------------------------

// Top-level globals defined per file. Each is a nullary function whose body fixes its inferred return
// type; the file then references the *previous* file's functions. Files are partitioned into independent
// chains of `CHAIN_LEN` (a chain head references nothing), so the cross-file dependency *depth* is
// bounded by `CHAIN_LEN` regardless of the package size — the realistic shape (real code imports a few
// modules a few hops deep, not an N-deep spine). A mid-chain body edit then has a fixed blast radius
// (`{edited file, edited file + 1}`) and a fixed interface-validation depth, both independent of N.
const ITEMS_PER_FILE: usize = 5;
const CHAIN_LEN: usize = 8;

// One file's source. `returns_double` flips every function's return literal `1L` (integer) -> `1.0`
// (double): a BODY-ONLY edit (the exported name set `g_i_*` is byte-for-byte identical) that nonetheless
// changes each function's inferred scheme, so the file's referrer genuinely re-typechecks — exercising
// the cross-file arm of the blast radius, not just the edited file in isolation.
fn generate_source(index: usize, returns_double: bool) -> String {
    let literal = if returns_double { "1.0" } else { "1L" };
    let mut source = String::new();
    for item in 0..ITEMS_PER_FILE {
        source.push_str(&format!("g_{index}_{item} <- function() {literal}\n"));
    }
    if index % CHAIN_LEN != 0 {
        let previous = index - 1;
        for item in 0..ITEMS_PER_FILE {
            source.push_str(&format!("u_{index}_{item} <- g_{previous}_{item}()\n"));
        }
    }
    source
}

// A file in the middle of its chain: it references the previous file (so editing it has a backward
// validation depth > 0) and has a successor in the same chain (so it has exactly one referrer). Used as
// the edit target so the blast radius is the representative `{m, m+1}`, not a chain endpoint special case.
fn mid_chain_edit_file(file_count: usize) -> FileId {
    let middle = file_count / 2;
    (middle - (middle % CHAIN_LEN) + CHAIN_LEN / 2) as FileId
}

// File 0 is `ITEMS_PER_FILE` lines; every other file is `2 * ITEMS_PER_FILE`. Size by the dominant term.
fn file_count_for_loc(target_loc: usize) -> usize {
    (target_loc / (2 * ITEMS_PER_FILE)).max(4)
}

fn total_loc(file_count: usize) -> usize {
    (0..file_count)
        .map(|index| generate_source(index, false).lines().count())
        .sum()
}

// ----------------------------------------------------------------------------------------------------
// New engine: build, warm, and measure a per-edit recheck
// ----------------------------------------------------------------------------------------------------

fn build_new_engine(file_count: usize) -> Engine<RoughlyQueries> {
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
        engine.set_input(
            Key::SourceText(index as FileId),
            generate_source(index, false),
        );
        engine.set_input(Key::DocumentKind(index as FileId), DocumentKind::Package);
    }
    engine
}

// The cold pass: fetch every file's diagnostics so every memo in the package is warm. After this the
// engine is in steady state and a subsequent edit measures pure incremental recheck cost.
fn warm_new_engine(engine: &Engine<RoughlyQueries>, file_count: usize) {
    for index in 0..file_count as FileId {
        let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(index));
    }
}

// Sum of per-file `Typecheck` body runs across the whole package — the count whose *delta* across an edit
// is the blast radius (how many files actually re-ran HM inference).
fn total_typecheck_runs(engine: &Engine<RoughlyQueries>, file_count: usize) -> u64 {
    (0..file_count as FileId)
        .map(|file| engine.group().typecheck_runs(file))
        .sum()
}

// Mean wall time to re-derive the edited file's and its referrer's diagnostics after a body edit,
// averaged over `rounds` toggling edits (each round flips the return type, a real body edit in both
// directions). Re-fetching just the edited file and its referrer is the editor's working set; the engine
// validates the rest lazily and only on demand.
fn measure_new_per_edit(
    engine: &mut Engine<RoughlyQueries>,
    edit_file: FileId,
    file_count: usize,
    rounds: usize,
) -> Duration {
    let referrer = edit_file + 1;
    let mut total = Duration::ZERO;
    for round in 0..rounds {
        let returns_double = round % 2 == 0;
        engine.set_input(
            Key::SourceText(edit_file),
            generate_source(edit_file as usize, returns_double),
        );
        let start = Instant::now();
        let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(edit_file));
        if (referrer as usize) < file_count {
            let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(referrer));
        }
        total += start.elapsed();
    }
    total / rounds as u32
}

// ----------------------------------------------------------------------------------------------------
// Old engine (production `analysis`): build, then measure a per-edit recheck
// ----------------------------------------------------------------------------------------------------

fn old_path(index: usize) -> PathBuf {
    PathBuf::from(format!("/pkg/R/file_{index:06}.R"))
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
            .add_document_from_source(old_path(index), &generate_source(index, false))
            .expect("benchmark source should parse");
    }
    run_full(&mut analysis_state);
    analysis_state
}

fn measure_old_per_edit(
    analysis_state: &mut Analysis,
    edit_file: usize,
    rounds: usize,
) -> Duration {
    let mut total = Duration::ZERO;
    for round in 0..rounds {
        let returns_double = round % 2 == 0;
        analysis_state
            .add_document_from_source(
                old_path(edit_file),
                &generate_source(edit_file, returns_double),
            )
            .expect("benchmark edit should parse");
        let start = Instant::now();
        let _ = analysis::typecheck(analysis_state);
        total += start.elapsed();
    }
    total / rounds as u32
}

// ----------------------------------------------------------------------------------------------------
// The committed correctness witness for the perf claim (runs by default, small N)
// ----------------------------------------------------------------------------------------------------

// Editing a function body must rerun work proportional to the edit's blast radius, not the package size:
// the lone all-files symbol fold does not re-fold, and exactly the edited file plus its single referrer
// re-typecheck. The exec counters are size-independent, so this holds at every N — the property the
// `#[ignore]` timing table then demonstrates as wall time.
#[test]
fn body_edit_recheck_is_blast_radius_bounded() {
    let file_count = 300;
    let edit_file: FileId = mid_chain_edit_file(file_count);
    let referrer = edit_file + 1;
    let control: FileId = edit_file + 40; // far from the edit; must stay on its cache.

    let mut engine = build_new_engine(file_count);
    warm_new_engine(&engine, file_count);

    let index_before = engine.group().package_symbol_index_runs();
    let type_defs_before = engine.group().package_type_definitions_runs();
    let typecheck_before = total_typecheck_runs(&engine, file_count);
    let typecheck_control_before = engine.group().typecheck_runs(control);

    // BODY-ONLY edit: every `g_{edit}_*` returns double instead of integer. Same exported name set.
    engine.set_input(
        Key::SourceText(edit_file),
        generate_source(edit_file as usize, true),
    );
    // Re-fetch EVERY file's diagnostics, so any file that *would* recompute does — the delta below is
    // then the true package-wide recompute set, not an artifact of which files we chose to fetch.
    warm_new_engine(&engine, file_count);

    let group = engine.group();
    // The headline: a body edit triggers ZERO package-wide *recomputation*. The names-only export set is
    // unchanged, so the all-files symbol fold does not re-run; no type declaration changed, so the all-files
    // type-definition fold does not re-run either (it folds the per-file declarations-only views, which cut
    // off). The only N-dependent per-edit cost left is the cheap *validation* walk, never an O(package) body.
    assert_eq!(
        group.package_symbol_index_runs(),
        index_before,
        "a body edit must trigger ZERO PackageSymbolIndex re-folds"
    );
    assert_eq!(
        group.package_type_definitions_runs(),
        type_defs_before,
        "a body edit must trigger ZERO PackageTypeDefinitions re-folds (declarations-only cutoff)"
    );
    // Exactly two files re-typecheck — the edited file and its referrer — regardless of the 300-file size.
    assert_eq!(
        total_typecheck_runs(&engine, file_count) - typecheck_before,
        2,
        "exactly the edited file and its single referrer re-typecheck (blast radius = 2)"
    );
    assert_eq!(
        group.typecheck_runs(edit_file),
        // edited file ran once cold + once now.
        2,
        "the edited file re-typechecks"
    );
    assert_eq!(
        group.typecheck_runs(referrer),
        2,
        "the referrer re-typechecks because the edited file's exported schemes changed"
    );
    assert_eq!(
        group.typecheck_runs(control),
        typecheck_control_before,
        "an unrelated file stays on its cache"
    );
}

// ----------------------------------------------------------------------------------------------------
// The perf table (heavy; manual)
// ----------------------------------------------------------------------------------------------------

#[test]
#[ignore = "perf benchmark; run manually with --release --nocapture"]
fn benchmark_new_vs_old_per_edit() {
    // 300k is included; if a size takes too long to build cold on a given machine, run a subset via
    // `BENCH_LOC` (comma-separated target LoC) — the 10k/100k rows already establish the trend.
    // `BENCH_ROUNDS` overrides the per-edit averaging count.
    let sizes: Vec<usize> = match std::env::var("BENCH_LOC") {
        Ok(value) => value
            .split(',')
            .filter_map(|entry| entry.trim().parse().ok())
            .collect(),
        Err(_) => vec![10_000usize, 100_000, 300_000],
    };
    let rounds: usize = std::env::var("BENCH_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(10);

    println!();
    println!(
        "  {:>8}  {:>7}  {:>13}  {:>13}  {:>8}   recompute(new)  PSI/PTD refolds",
        "LoC", "files", "new per-edit", "old per-edit", "ratio"
    );
    for target_loc in sizes {
        let file_count = file_count_for_loc(target_loc);
        let loc = total_loc(file_count);
        let edit_file = mid_chain_edit_file(file_count);

        // NEW engine.
        let mut engine = build_new_engine(file_count);
        warm_new_engine(&engine, file_count);
        let typecheck_before = total_typecheck_runs(&engine, file_count);
        let index_before = engine.group().package_symbol_index_runs();
        let type_defs_before = engine.group().package_type_definitions_runs();
        let new_per_edit = measure_new_per_edit(&mut engine, edit_file, file_count, rounds);
        // Per-edit recompute / refold counts: divide the deltas by the number of edits.
        let recompute_per_edit =
            (total_typecheck_runs(&engine, file_count) - typecheck_before) / rounds as u64;
        let index_refolds_per_edit =
            (engine.group().package_symbol_index_runs() - index_before) / rounds as u64;
        let type_defs_refolds_per_edit =
            (engine.group().package_type_definitions_runs() - type_defs_before) / rounds as u64;

        // OLD engine.
        let mut analysis_state = build_old_engine(file_count);
        let old_per_edit = measure_old_per_edit(&mut analysis_state, edit_file as usize, rounds);

        let new_ms = new_per_edit.as_secs_f64() * 1e3;
        let old_ms = old_per_edit.as_secs_f64() * 1e3;
        let ratio = old_ms / new_ms;
        println!(
            "  {loc:>8}  {file_count:>7}  {new_ms:>10.3} ms  {old_ms:>10.3} ms  {ratio:>6.1}x   \
             {recompute_per_edit:>13}   {index_refolds_per_edit}/{type_defs_refolds_per_edit}"
        );
    }
    println!();
}
