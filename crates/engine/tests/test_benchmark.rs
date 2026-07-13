//! The O(blast-radius) per-edit benchmark (`DESIGN.md` §8): per-edit cost is measured O(blast-radius) and
//! compared against the from-scratch `analysis` path.
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
//! 2. **Wall time is ~90–105× lower than production at every size, but both still scale ~linearly in
//!    N.** The engine is *not* flat in wall time: confirming the package-wide folds
//!    (`PackageSymbolIndex`, `PackageTypeDefinitions`, the package-naming candidate order) are unchanged
//!    walks each fold's own dependency edges. Durability bounds that walk — an open-document keystroke is
//!    a `LOW` change, so every unopened file's chain answers green in O(1) without recursing to its
//!    inputs; what remains is the fold edges themselves, an O(1) check each. Driving *that* sub-linear
//!    (a durable sub-fold over unopened files plus an open-file overlay, giving O(open) validation) is
//!    the recorded next lever (`DESIGN.md` §8).
//!    Production's O(package) term, by contrast, is HM re-inference plus a per-round interface-table
//!    rebuild and a type-definition-fingerprint render over every module — far costlier per unit N.
//!    (Allocation churn per edit, unlike wall time, IS flat in N — see `test_memory.rs`'s
//!    `churn_per_body_edit`.)
//!
//! Measured here (release, mid-chain body edit on an open document, durable corpus; your hardware will
//! differ, but the *shape* is stable):
//!
//! ```text
//!     LoC    files   new per-edit   old per-edit    ratio   recompute  PSI/PTD refolds
//!    9 375    1 000       2.2 ms        232 ms     105.4x       2        0 / 0
//!   93 750   10 000      28.1 ms      2 812 ms      99.9x       2        0 / 0
//!  281 250   30 000      91.2 ms      8 342 ms      91.5x       2        0 / 0
//! ```
//!
//! The IDE read latencies on the same workspace ([`benchmark_ide_read_latency`], 30 000 files): hover
//! and definition are ~0.01–0.03 ms at rest; the first read after a keystroke pays the fold-edge walk
//! (hover ~44 ms, definition ~20 ms at this pathological file count — real 300k-LoC workspaces have
//! 10–60× fewer files, scaling the walk down proportionally).
//!
//! # Layout
//!
//! - [`body_edit_recheck_is_blast_radius_bounded`] runs by default (small N): it is the exec-counter
//!   proof that the recompute set is blast-radius bounded and the index does not re-fold. This is the
//!   committed correctness witness for the perf claim.
//! - [`ide_reads_at_rest_touch_constant_validation_work`] and
//!   [`open_file_edit_walk_is_durability_bounded`] run by default: the committed latency-regression
//!   witnesses (constant at-rest prime scope; durability-bounded post-keystroke walk).
//! - [`benchmark_new_vs_old_per_edit`] and [`benchmark_ide_read_latency`] are `#[ignore]` (heavy): the
//!   wall-clock tables at 10k / 100k / 300k LoC. Run them manually:
//!   `cargo test -p engine --release --test test_benchmark -- --ignored --nocapture`.

use {
    analysis::{Analysis, CheckConfig, LintConfig, naming::DocumentKind, run_full},
    engine::{
        Durability, Engine,
        queries::{Config, FileDiagnostics, FileId, Key, RoughlyQueries, parse_source_input},
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

// Builds one file's source for a given file index; the benchmark scenarios pair two of these to
// toggle the edit target between rounds.
type SourceBuilder = Box<dyn Fn(usize) -> String>;

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
    if !index.is_multiple_of(CHAIN_LEN) {
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

// The on-disk path both engines report for a file — kept identical so cross-file diagnostic notes
// (which embed the neighbour's path) stay byte-comparable between the two pipelines.
fn package_path(index: usize) -> PathBuf {
    PathBuf::from(format!("/pkg/R/file_{index:06}.R"))
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

// Mirrors the language server's input feeding: the on-disk corpus and project facts are durable;
// only the file a scenario edits is later downgraded to an open document (see [`open_file_edit`]).
fn build_new_engine(file_count: usize) -> Engine<RoughlyQueries> {
    let mut engine = Engine::new(RoughlyQueries::new());
    engine.set_input_durable(
        Key::ProjectFiles,
        (0..file_count as FileId).collect::<Vec<_>>(),
        Durability::HIGH,
    );
    engine.set_input_durable(
        Key::Config,
        Config {
            check: CheckConfig {
                typing: true,
                strict: false,
                unused: false,
            },
            lint: LintConfig::default(),
        },
        Durability::HIGH,
    );
    for index in 0..file_count {
        engine.set_input_durable(
            Key::SourceText(index as FileId),
            parse_source_input(&generate_source(index, false)),
            Durability::HIGH,
        );
        engine.set_input_durable(
            Key::DocumentKind(index as FileId),
            DocumentKind::Package,
            Durability::HIGH,
        );
        engine.set_input_durable(
            Key::FileName(index as FileId),
            package_path(index),
            Durability::HIGH,
        );
    }
    engine
}

// An editor keystroke: the open document's text is the one low-durability input, exactly as the
// language server feeds it.
fn open_file_edit(engine: &mut Engine<RoughlyQueries>, file: FileId, source: &str) {
    engine.set_input_durable(
        Key::SourceText(file),
        parse_source_input(source),
        Durability::LOW,
    );
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
        open_file_edit(
            engine,
            edit_file,
            &generate_source(edit_file as usize, returns_double),
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
            .add_document_from_source(package_path(index), &generate_source(index, false))
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
                package_path(edit_file),
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
        parse_source_input(&generate_source(edit_file as usize, true)),
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
// Invalidation cliffs: the two edit shapes that are NOT blast-radius bounded
// ----------------------------------------------------------------------------------------------------

// A syntactically broken keystroke state: lowering short-circuits a malformed tree to an empty
// module, so the file's exported names flip to empty (and back on the fixing keystroke).
fn malformed_source(index: usize) -> String {
    let mut source = generate_source(index, false);
    source.push_str("g_broken <- function() (\n");
    source
}

// The committed witness for the malformed-flip behavior: error-tolerant lowering salvages the
// well-formed definitions of a file whose tail is mid-edit, so entering and leaving a broken state
// keeps the exported-name set IDENTICAL — the all-files symbol index does not re-fold at all, the
// edited file's own schemes revalidate equal (early cutoff), and no other file recomputes. During
// the broken window the referrer keeps resolving the edited file's names: a half-typed construct
// no longer floods dependents with unresolved-name errors.
#[test]
fn malformed_flip_keeps_exports_stable_and_recompute_bounded() {
    let file_count = 300;
    let edit_file: FileId = mid_chain_edit_file(file_count);

    let mut engine = build_new_engine(file_count);
    warm_new_engine(&engine, file_count);

    let index_before = engine.group().package_symbol_index_runs();
    let typecheck_before = total_typecheck_runs(&engine, file_count);

    // Enter the malformed state (mid-keystroke), then leave it (the fixing keystroke), fetching
    // everything after each flip so every file that would recompute does.
    engine.set_input(
        Key::SourceText(edit_file),
        parse_source_input(&malformed_source(edit_file as usize)),
    );
    warm_new_engine(&engine, file_count);
    let referrer = edit_file + 1;
    let broken_window = engine.fetch::<FileDiagnostics>(Key::Diagnostics(referrer));
    assert!(
        broken_window.package_naming.is_empty() && broken_window.type_errors.is_empty(),
        "the referrer keeps resolving the edited file's exports while its tail is broken"
    );
    engine.set_input(
        Key::SourceText(edit_file),
        parse_source_input(&generate_source(edit_file as usize, false)),
    );
    warm_new_engine(&engine, file_count);

    let group = engine.group();
    assert_eq!(
        group.package_symbol_index_runs() - index_before,
        0,
        "a malformed round-trip leaves the exported-name set unchanged, so the all-files \
         symbol index never re-folds"
    );
    let recompute = total_typecheck_runs(&engine, file_count) - typecheck_before;
    assert_eq!(
        recompute, 2,
        "a malformed round-trip re-typechecks only the edited file per side (schemes cut off \
         before the referrer)"
    );
}

// The cliff timing table: what the two not-blast-radius-bounded edit shapes actually cost, next to a
// plain body edit at the same size. `working set` fetches the edited file + referrer (the editor's
// synchronous cost); `full sweep` re-fetches every file (a pull-everything client, and the upper
// bound on deferred cost). Sizes via `BENCH_LOC`, rounds via `BENCH_ROUNDS` as above.
#[test]
#[ignore = "perf benchmark; run manually with --release --nocapture"]
fn benchmark_invalidation_cliffs() {
    let target_loc: usize = std::env::var("BENCH_LOC")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(100_000);
    let rounds: usize = std::env::var("BENCH_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(10);
    let file_count = file_count_for_loc(target_loc);
    let edit_file = mid_chain_edit_file(file_count);
    println!();
    println!(
        "  {} files ({} LoC), mid-chain edit target, {rounds} rounds",
        file_count,
        total_loc(file_count)
    );
    println!(
        "  {:<24}  {:>12}  {:>12}  {:>15}  {:>11}",
        "scenario", "working set", "full sweep", "recompute/edit", "PSI/edit"
    );

    // Each scenario toggles between two sources for the edit target; per round: set input, fetch the
    // working set (timed), then sweep the rest (timed separately).
    let scenarios: [(&str, SourceBuilder, SourceBuilder); 3] = [
        (
            "body edit",
            Box::new(|index| generate_source(index, false)),
            Box::new(|index| generate_source(index, true)),
        ),
        (
            "malformed flip",
            Box::new(|index| generate_source(index, false)),
            Box::new(malformed_source),
        ),
        (
            "re-export retarget",
            Box::new(|index| reexport_source(index, false)),
            Box::new(|index| reexport_source(index, true)),
        ),
    ];

    for (name, source_a, source_b) in scenarios {
        let mut engine = build_new_engine(file_count);
        open_file_edit(&mut engine, edit_file, &source_a(edit_file as usize));
        warm_new_engine(&engine, file_count);

        let typecheck_before = total_typecheck_runs(&engine, file_count);
        let index_before = engine.group().package_symbol_index_runs();
        let mut working_set = Duration::ZERO;
        let mut full_sweep = Duration::ZERO;
        for round in 0..rounds {
            let source = if round % 2 == 0 {
                source_b(edit_file as usize)
            } else {
                source_a(edit_file as usize)
            };
            open_file_edit(&mut engine, edit_file, &source);
            let start = Instant::now();
            let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(edit_file));
            let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(edit_file + 1));
            working_set += start.elapsed();
            let start = Instant::now();
            warm_new_engine(&engine, file_count);
            full_sweep += start.elapsed();
        }
        let recompute_per_edit =
            (total_typecheck_runs(&engine, file_count) - typecheck_before) as f64 / rounds as f64;
        let index_per_edit =
            (engine.group().package_symbol_index_runs() - index_before) as f64 / rounds as f64;
        println!(
            "  {name:<24}  {:>9.3} ms  {:>9.3} ms  {recompute_per_edit:>15.1}  {index_per_edit:>11.1}",
            (working_set / rounds as u32).as_secs_f64() * 1e3,
            (full_sweep / rounds as u32).as_secs_f64() * 1e3,
        );
    }
    println!();
}

// Like `generate_source`, plus one re-export line whose *target* flips with `retargeted`: the
// exported name set is unchanged, but the re-export graph edge moves — the reference-set edit shape
// that re-walks the symbol-SCC machinery rather than just re-typechecking bodies.
fn reexport_source(index: usize, retargeted: bool) -> String {
    let mut source = generate_source(index, false);
    if index > 0 {
        let target_item = if retargeted { 1 } else { 0 };
        source.push_str(&format!("alias_{index} <- g_{}_{target_item}\n", index - 1));
    }
    source
}

// ----------------------------------------------------------------------------------------------------
// The perf table (heavy; manual)
// ----------------------------------------------------------------------------------------------------

// ----------------------------------------------------------------------------------------------------
// IDE read latency (heavy; manual): what a hover/completion/definition/references request costs on a
// large package, both at rest (engine fully green) and as the first read after a body edit (which pays
// the revision's validation walk plus the blast-radius recompute). These are the numbers an editor user
// feels per keystroke; the per-edit diagnostics table above measures the publish path instead.
// ----------------------------------------------------------------------------------------------------

#[test]
#[ignore = "perf benchmark; run manually with --release --nocapture"]
fn benchmark_ide_read_latency() {
    use {
        analysis::text::TextPosition,
        engine::ide_view::{EngineIde, PathTable},
    };

    let sizes: Vec<usize> = match std::env::var("BENCH_LOC") {
        Ok(value) => value
            .split(',')
            .filter_map(|entry| entry.trim().parse().ok())
            .collect(),
        Err(_) => vec![100_000usize, 300_000],
    };
    let rounds: usize = std::env::var("BENCH_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(6);

    for target_loc in sizes {
        let file_count = file_count_for_loc(target_loc);
        let edit_file = mid_chain_edit_file(file_count);
        let referrer = edit_file + 1;
        let referrer_path = package_path(referrer as usize);

        // The referrer's first `u_` line is `u_{referrer}_0 <- g_{edit_file}_0()`; the read target is
        // the `g_…` reference (hover/definition inside it, completion at its end).
        let reference_column = format!("u_{referrer}_0 <- ").chars().count();
        let reference_length = format!("g_{edit_file}_0").chars().count();
        let inside_reference = TextPosition {
            line_index: ITEMS_PER_FILE,
            character_index: reference_column + 1,
        };
        let end_of_reference = TextPosition {
            line_index: ITEMS_PER_FILE,
            character_index: reference_column + reference_length,
        };

        let mut engine = build_new_engine(file_count);
        let mut paths = PathTable::new(PathBuf::from("/pkg"));
        for index in 0..file_count {
            paths.insert(index as FileId, package_path(index));
        }

        let start = Instant::now();
        warm_new_engine(&engine, file_count);
        let cold = start.elapsed();

        println!();
        println!(
            "  {file_count} files ({} LoC), reads on the mid-chain referrer, {rounds} rounds",
            total_loc(file_count)
        );
        println!(
            "    cold full analysis            {:>10.1} ms",
            cold.as_secs_f64() * 1e3
        );

        // At rest: every memo green at the current revision, so a read pays only its own prime scope.
        let measure_at_rest = |name: &str, read: &dyn Fn(&EngineIde)| {
            let ide = EngineIde::new(&engine, &paths);
            read(&ide); // warm the per-revision validation once, off the clock
            let start = Instant::now();
            for _ in 0..rounds {
                read(&ide);
            }
            let per_read = start.elapsed() / rounds as u32;
            println!(
                "    at rest     {name:<12}      {:>10.3} ms",
                per_read.as_secs_f64() * 1e3
            );
        };
        measure_at_rest("hover", &|ide| {
            let _ = ide.hover(&referrer_path, inside_reference);
        });
        measure_at_rest("completion", &|ide| {
            let _ = ide.completion(&referrer_path, end_of_reference);
        });
        measure_at_rest("definition", &|ide| {
            let _ = ide.definition(&referrer_path, inside_reference);
        });
        measure_at_rest("references", &|ide| {
            let _ = ide.references(&referrer_path, inside_reference, true);
        });

        // After a body edit: the first read of the new revision pays the whole validation walk plus the
        // recompute of whatever it primes (here `Typecheck(referrer)`, whose dependencies include the
        // edited file's schemes). This is the latency a user feels asking for hover while typing.
        let mut measure_after_edit =
            |name: &str, read: &dyn Fn(&EngineIde, &Engine<RoughlyQueries>)| {
                let mut total = Duration::ZERO;
                for round in 0..rounds {
                    open_file_edit(
                        &mut engine,
                        edit_file,
                        &generate_source(edit_file as usize, round % 2 == 0),
                    );
                    let ide = EngineIde::new(&engine, &paths);
                    let start = Instant::now();
                    read(&ide, &engine);
                    total += start.elapsed();
                }
                let per_read = total / rounds as u32;
                println!(
                    "    after edit  {name:<12}      {:>10.3} ms",
                    per_read.as_secs_f64() * 1e3
                );
            };
        measure_after_edit("psi walk", &|_, engine| {
            let _ = engine.fetch::<analysis::naming::NamesGlobal>(Key::PackageSymbolIndex);
        });
        measure_after_edit("hover", &|ide, _| {
            let _ = ide.hover(&referrer_path, inside_reference);
        });
        measure_after_edit("completion", &|ide, _| {
            let _ = ide.completion(&referrer_path, end_of_reference);
        });
        measure_after_edit("definition", &|ide, _| {
            let _ = ide.definition(&referrer_path, inside_reference);
        });
    }
    println!();
}

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

// ----------------------------------------------------------------------------------------------------
// Committed latency-regression witnesses (run by default, small N — the properties are structural, so
// they hold at every size; the `#[ignore]` tables above put wall-clock numbers on the same claims)
// ----------------------------------------------------------------------------------------------------

// At rest — engine fully validated at the current revision — an IDE read touches a bounded number of
// memos: its own prime scope, independent of the package size. This is the anti-regression for the
// completion source specifically: completion must serve globals from the one memoized package fold,
// never by touching every file's module and naming (which made a single at-rest completion O(package)).
#[test]
fn ide_reads_at_rest_touch_constant_validation_work() {
    use {
        analysis::text::TextPosition,
        engine::ide_view::{EngineIde, PathTable},
    };

    let file_count = 300;
    let edit_file = mid_chain_edit_file(file_count);
    let referrer = edit_file + 1;
    let referrer_path = package_path(referrer as usize);
    let engine = build_new_engine(file_count);
    warm_new_engine(&engine, file_count);
    let mut paths = PathTable::new(PathBuf::from("/pkg"));
    for index in 0..file_count {
        paths.insert(index as FileId, package_path(index));
    }
    let reference_column = format!("u_{referrer}_0 <- ").chars().count();
    let inside_reference = TextPosition {
        line_index: ITEMS_PER_FILE,
        character_index: reference_column + 1,
    };

    // First requests build the request-scoped indexes (the completion fold's per-file entries run
    // once); the steady state under test is every request after that.
    let ide = EngineIde::new(&engine, &paths);
    let _ = ide.hover(&referrer_path, inside_reference);
    let _ = ide.completion(&referrer_path, inside_reference);
    let _ = ide.definition(&referrer_path, inside_reference);

    let assert_bounded = |name: &str, read: &dyn Fn(&EngineIde)| {
        let before = engine.validation_count();
        read(&EngineIde::new(&engine, &paths));
        let touched = engine.validation_count() - before;
        assert!(
            touched <= 32,
            "an at-rest {name} touched {touched} memos; its prime scope must be constant, \
             not O(package)"
        );
    };
    assert_bounded("hover", &|ide| {
        let _ = ide.hover(&referrer_path, inside_reference);
    });
    assert_bounded("completion", &|ide| {
        let _ = ide.completion(&referrer_path, inside_reference);
    });
    assert_bounded("definition", &|ide| {
        let _ = ide.definition(&referrer_path, inside_reference);
    });
}

// After an open-document keystroke, the first read of the new revision walks the all-files folds'
// edges (each an O(1) durability check) plus the edited file's own chain — but never the unopened
// files' chains. The bound is a small multiple of N; without durability the walk recursed through
// every file's parse/lower/naming chain and sat several times higher. (Measured shape at this N:
// ~2.6k touched with durability, ~7.5k+ without.)
#[test]
fn open_file_edit_walk_is_durability_bounded() {
    let file_count = 300;
    let edit_file = mid_chain_edit_file(file_count);
    let referrer = edit_file + 1;
    let mut engine = build_new_engine(file_count);
    warm_new_engine(&engine, file_count);

    // Open the edit target (downgrade, same text), and settle the one-time flush walk.
    open_file_edit(
        &mut engine,
        edit_file,
        &generate_source(edit_file as usize, false),
    );
    warm_new_engine(&engine, file_count);

    // The keystroke: a genuine body edit on the open document.
    open_file_edit(
        &mut engine,
        edit_file,
        &generate_source(edit_file as usize, true),
    );
    let before = engine.validation_count();
    let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(edit_file));
    let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(referrer));
    let touched = engine.validation_count() - before;
    assert!(
        touched as usize <= 12 * file_count,
        "a post-keystroke diagnostics read touched {touched} memos at {file_count} files; \
         the unopened files' chains must validate O(1) via durability, leaving only the \
         fold edges and the edited chain"
    );
}
