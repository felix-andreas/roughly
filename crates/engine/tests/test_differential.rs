//! R2 — the differential correctness gate (`DESIGN.md` §7).
//!
//! # What this proves
//!
//! The new engine, driven *incrementally* through an edit stream, produces the same diagnostics as the
//! production `analysis` crate's **full from-scratch rebuild** of the *final* state after every edit. The
//! oracle is deliberately `analysis`'s `run_full` on a freshly built `Analysis` for the current file set —
//! never `analysis`'s own incremental path — because the incremental path is exactly what carried the
//! silent-stale bug class the rewrite exists to remove; comparing against it could ratify a stale result
//! on both sides. The production debug drift oracles run inside `run_full`, so the oracle is self-checked.
//!
//! # Comparison scope (decided and documented, per the R2 brief)
//!
//! After each edit, for every current file, the engine's `Diagnostics(f)` must equal the oracle's
//! `document_diagnostics(f)` as a **normalized set** (sorted by range, then code, then severity, then
//! message). The two engines use independent interners, so the comparison is over *rendered* facts (byte
//! range + code + severity + message), which are interner-independent.
//!
//! We compare the **type-error and strict-mode** diagnostic classes (`DiagnosticCode::TypeError` and
//! `DiagnosticCode::Strict`). These are exactly the classes the engine's novel per-symbol type-interface
//! layer and ported strict-origin rendering produce, and the three R1a deferrals (real stub library,
//! package-global type definitions, cross-file ranges) all manifest as differences in *these* classes.
//! The other classes are excluded from **both** sides, for reasons that are representation facts, not hidden
//! divergences:
//!
//! - **Lint** (`DiagnosticCode::Lint`): the engine does not model lint at all.
//! - **Local naming** (`DiagnosticCode::Naming` from `resolve_document_locally`): produced by the engine
//!   by calling the *same* `analysis` function verbatim with the *same* inputs, so it is identical by
//!   construction and exercises no engine-specific logic.
//! - **Package naming** (`DiagnosticCode::Naming`/`SyntaxError` from `package_document_diagnostics`: the
//!   "could not resolve", builtin/namespace-shadow, overwrite, duplicate-type, and type-reference
//!   diagnostics): this is `analysis`'s cross-file package-naming subsystem, which is **`pub(crate)`** and
//!   therefore unreachable from the engine crate via the existing public API. The engine does not yet port
//!   a package-naming *query*, so this class is an acknowledged, characterized representation gap — not a
//!   bug in ported logic. (`DESIGN.md` §3's `PackageSymbolIndex` is names-only and emits no diagnostics.)
//! - **Annotation errors** (`DiagnosticCode::AnnotationError`): a lowering/naming-phase class;
//!   `Diagnostic::from_inference_error` never emits this code, so it is outside the engine's type-error
//!   surface. The generators below emit only well-formed annotations, so this class does not arise anyway.
//!
//! Excluding a class is done **symmetrically** (the same filter on both sides) and by **code**, never by
//! message, so it cannot mask a divergence inside the compared classes: any type/strict diagnostic the
//! engine produces that the oracle does not (or vice versa) fails the assertion immediately.
//!
//! # Drivers
//!
//! 1. A curated set of deterministic scenarios mirroring the `analysis` cross-file fixtures (cross-file
//!    function/value use and mismatch, cross-file `@alias`/`@type`, stdlib base-name use, re-export chains
//!    and a period-2 cycle, an interface-change-creates-error incremental edit, add→delete→re-add of the
//!    same path, package↔script reclassification, and a global rename).
//! 2. A randomized, fixed-seed generator over a small realistic R alphabet producing multi-file
//!    package+script workspaces and adversarial edit streams (edit→fetch→edit, add/delete/re-add the same
//!    slot, reclassification, cross-file errors introduced and resolved, strict toggling). Parity is
//!    asserted after **every** step; the seed is fixed so a failure is a deterministic repro.

use {
    analysis::{
        Analysis, CheckConfig, Diagnostic, DiagnosticCode, LintConfig, Severity,
        naming::DocumentKind, run_full,
    },
    engine::{
        Engine,
        queries::{Config, FileDiagnostics, FileId, Key, RoughlyQueries},
    },
    std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    },
    tree_sitter::Range,
};

// ----------------------------------------------------------------------------------------------------
// Workspace model
// ----------------------------------------------------------------------------------------------------

#[derive(Clone)]
struct FileState {
    source: String,
    package: bool,
}

#[derive(Clone)]
struct Workspace {
    // Keyed by a stable slot id. A slot's path is a pure function of (id, package), so deleting then
    // re-adding the same slot reuses its path — exercising the engine's tombstone re-add.
    files: BTreeMap<FileId, FileState>,
    config: Config,
}

impl Workspace {
    fn new(config: Config) -> Workspace {
        Workspace {
            files: BTreeMap::new(),
            config,
        }
    }
}

const BASE: &str = "/ws";

// The path of a slot, package-relative key embedded so package files live under `<base>/R/...` (which is
// what `Analysis::is_package_path` keys on) and scripts do not. Zero-padded so the lexical path order the
// engine feeds `ProjectFiles` matches the `package_path_key` order production sorts package documents by.
fn relative_key(id: FileId, package: bool) -> String {
    if package {
        format!("R/f{id:04}.R")
    } else {
        format!("s{id:04}.R")
    }
}

fn absolute_path(id: FileId, package: bool) -> PathBuf {
    PathBuf::from(format!("{BASE}/{}", relative_key(id, package)))
}

// The `ProjectFiles` order: every current file id, sorted by its package-relative key so the engine's
// last-writer-wins fold and production's path-last winner agree.
fn project_files_order(workspace: &Workspace) -> Vec<FileId> {
    let mut entries = workspace
        .files
        .iter()
        .map(|(id, state)| (relative_key(*id, state.package), *id))
        .collect::<Vec<_>>();
    entries.sort();
    entries.into_iter().map(|(_, id)| id).collect()
}

// ----------------------------------------------------------------------------------------------------
// Oracle: a fresh full rebuild of the current state
// ----------------------------------------------------------------------------------------------------

fn check_config(config: &Config) -> CheckConfig {
    CheckConfig {
        unused: config.unused,
        typing: config.typing,
        strict: config.strict,
    }
}

fn build_oracle(workspace: &Workspace) -> Analysis {
    let mut analysis = Analysis::new(
        PathBuf::from(BASE),
        LintConfig::default(),
        check_config(&workspace.config),
    );
    // Add in deterministic key order. Order does not affect `run_full`'s result, but keeping it stable
    // keeps any debug-oracle failure reproducible.
    for id in project_files_order(workspace) {
        let state = &workspace.files[&id];
        analysis
            .add_document_from_source(absolute_path(id, state.package), &state.source)
            .expect("oracle: current source should parse");
    }
    run_full(&mut analysis);
    analysis
}

fn oracle_diagnostics(analysis: &Analysis, id: FileId, package: bool) -> Vec<Diagnostic> {
    let path = absolute_path(id, package);
    let document_id = analysis
        .document_id_for_path(Path::new(&path))
        .expect("oracle: document for current file should exist");
    analysis
        .document_diagnostics(document_id)
        .into_iter()
        .filter(is_compared)
        .collect()
}

// ----------------------------------------------------------------------------------------------------
// System under test: one long-lived engine, updated incrementally
// ----------------------------------------------------------------------------------------------------

fn sync_engine(engine: &mut Engine<RoughlyQueries>, previous: &Workspace, next: &Workspace) {
    // Retract files that no longer exist (delete): drop both per-file inputs, leaving the engine's
    // tombstones so the folds revalidate against the smaller set.
    for id in previous.files.keys() {
        if !next.files.contains_key(id) {
            engine.remove_input(&Key::SourceText(*id));
            engine.remove_input(&Key::DocumentKind(*id));
        }
    }
    // Set every current file's inputs. Re-setting an unchanged input is cheap (value-eq backdating leaves
    // dependents green), so unconditionally setting the whole current state each step is correct and also
    // exercises the no-op cutoff path.
    for (id, state) in &next.files {
        engine.set_input(Key::SourceText(*id), state.source.clone());
        engine.set_input(
            Key::DocumentKind(*id),
            if state.package {
                DocumentKind::Package
            } else {
                DocumentKind::Script
            },
        );
    }
    engine.set_input(Key::ProjectFiles, project_files_order(next));
    engine.set_input(Key::Config, next.config.clone());
}

fn engine_diagnostics(engine: &Engine<RoughlyQueries>, id: FileId, config: &Config) -> Vec<Diagnostic> {
    let file_diagnostics = engine.fetch::<FileDiagnostics>(Key::Diagnostics(id));
    let fallback = *engine.fetch::<Range>(Key::FallbackRange);
    let mut rendered = Vec::new();
    if config.typing {
        // Render the raw inference errors against the engine's own interner + fallback range, exactly as
        // production's `typecheck` renders them.
        engine.group().with_interner(|interner| {
            for error in &file_diagnostics.type_errors {
                rendered.push(Diagnostic::from_inference_error(error, fallback, interner));
            }
        });
    }
    if config.strict {
        rendered.extend(file_diagnostics.strict_diagnostics.iter().cloned());
    }
    rendered.into_iter().filter(is_compared).collect()
}

// ----------------------------------------------------------------------------------------------------
// Normalization + the parity assertion
// ----------------------------------------------------------------------------------------------------

// The compared classes: the type-error and strict-mode diagnostics. See the module doc for why the other
// classes are excluded symmetrically from both sides.
fn is_compared(diagnostic: &Diagnostic) -> bool {
    matches!(
        diagnostic.code,
        DiagnosticCode::TypeError | DiagnosticCode::Strict
    )
}

type NormalizedDiagnostic = (usize, usize, u8, u8, String);

fn code_rank(code: DiagnosticCode) -> u8 {
    match code {
        DiagnosticCode::Lint => 0,
        DiagnosticCode::Naming => 1,
        DiagnosticCode::SyntaxError => 2,
        DiagnosticCode::TypeError => 3,
        DiagnosticCode::AnnotationError => 4,
        DiagnosticCode::Strict => 5,
    }
}

fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
    }
}

fn normalize(diagnostics: Vec<Diagnostic>) -> Vec<NormalizedDiagnostic> {
    let mut normalized = diagnostics
        .into_iter()
        .map(|diagnostic| {
            (
                diagnostic.range.start_byte,
                diagnostic.range.end_byte,
                code_rank(diagnostic.code),
                severity_rank(diagnostic.severity),
                diagnostic.message,
            )
        })
        .collect::<Vec<_>>();
    normalized.sort();
    normalized
}

fn assert_parity(label: &str, engine: &Engine<RoughlyQueries>, workspace: &Workspace) {
    let oracle = build_oracle(workspace);
    for (id, state) in &workspace.files {
        let engine_set = normalize(engine_diagnostics(engine, *id, &workspace.config));
        let oracle_set = normalize(oracle_diagnostics(&oracle, *id, state.package));
        assert_eq!(
            engine_set, oracle_set,
            "parity divergence at step `{label}` for file {id} (path {:?})\n\
             engine: {engine_set:#?}\noracle: {oracle_set:#?}\nsource:\n{}",
            absolute_path(*id, state.package),
            state.source,
        );
    }
}

// A driver that owns the long-lived engine and the current workspace, asserting parity after each step.
struct Driver {
    engine: Engine<RoughlyQueries>,
    workspace: Workspace,
}

impl Driver {
    fn new(config: Config) -> Driver {
        Driver {
            engine: Engine::new(RoughlyQueries::new()),
            workspace: Workspace::new(config),
        }
    }

    fn step(&mut self, label: &str, mutate: impl FnOnce(&mut Workspace)) {
        let previous = self.workspace.clone();
        mutate(&mut self.workspace);
        sync_engine(&mut self.engine, &previous, &self.workspace);
        assert_parity(label, &self.engine, &self.workspace);
    }

    fn set_package(&mut self, label: &str, id: FileId, source: &str) {
        let source = source.to_owned();
        self.step(label, |workspace| {
            workspace.files.insert(
                id,
                FileState {
                    source,
                    package: true,
                },
            );
        });
    }

    fn set_script(&mut self, label: &str, id: FileId, source: &str) {
        let source = source.to_owned();
        self.step(label, |workspace| {
            workspace.files.insert(
                id,
                FileState {
                    source,
                    package: false,
                },
            );
        });
    }

    fn delete(&mut self, label: &str, id: FileId) {
        self.step(label, |workspace| {
            workspace.files.remove(&id);
        });
    }

    fn set_strict(&mut self, label: &str, strict: bool) {
        self.step(label, |workspace| {
            workspace.config.strict = strict;
        });
    }
}

fn typing_config() -> Config {
    Config {
        typing: true,
        strict: false,
        unused: false,
    }
}

fn typing_strict_config() -> Config {
    Config {
        typing: true,
        strict: true,
        unused: false,
    }
}

// ----------------------------------------------------------------------------------------------------
// Curated deterministic scenarios (mirroring the `analysis` cross-file fixtures)
// ----------------------------------------------------------------------------------------------------

#[test]
fn cross_file_function_use_and_argument_mismatch() {
    let mut driver = Driver::new(typing_config());
    driver.set_package(
        "define-fn",
        0,
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count",
    );
    driver.set_package("use-ok", 1, "result <- double_count(2L)");
    // Introduce a cross-file argument mismatch by editing the referrer.
    driver.set_package("use-bad", 1, "result <- double_count(\"two\")");
    // Resolve it again.
    driver.set_package("use-ok-again", 1, "result <- double_count(2L)");
}

#[test]
fn cross_file_value_annotation_mismatch() {
    let mut driver = Driver::new(typing_config());
    driver.set_package("define-int", 0, "count <- 1L");
    driver.set_package("annotate-mismatch", 1, "#: character\nlabel <- count");
    // Fix the definition's type so the annotation now holds.
    driver.set_package("fix-definition", 0, "count <- \"one\"");
}

#[test]
fn cross_file_alias_use_resolves_via_package_type_definitions() {
    // Deferral 2: an `@alias` defined in one file, referenced in another. Without the package-global
    // type-definition query the referrer cannot resolve `Count` and raises a type error.
    let mut driver = Driver::new(typing_config());
    driver.set_package("use-before-def", 1, "#: Count\nvalue <- 1L");
    // At this point `Count` is undefined package-wide: both engine and oracle report the same type error.
    driver.set_package("define-alias", 0, "#: @alias Count {integer}");
    // Now the alias resolves cross-file: no diagnostics.
    // Break it: the alias now expands to character, so the integer value mismatches.
    driver.set_package("retarget-alias", 0, "#: @alias Count {character}");
    // Remove the alias entirely; the reference dangles again.
    driver.delete("delete-alias", 0);
}

#[test]
fn cross_file_nominal_new_and_use() {
    // Deferral 2 again, with a nominal `@type` and structural-rejection behavior.
    let mut driver = Driver::new(typing_config());
    driver.set_package(
        "define-nominal",
        0,
        "#: @type Person {list{name: character}}\n\n#: fn(value: Person) -> character\nget_name <- function(value) value$name",
    );
    driver.set_package(
        "construct-and-use",
        1,
        "#: @new Person\nperson <- list(name = \"bob\")\nresult <- get_name(person)",
    );
    // A structural value (no @new) is rejected where a nominal is expected.
    driver.set_package("structural-rejected", 1, "result <- get_name(list(name = \"bob\"))");
}

#[test]
fn stdlib_base_name_use_resolves_via_real_stubs() {
    // Deferral 1: base names resolve to their stub schemes, so a well-typed `nchar` use checks clean and a
    // mis-typed one errors — neither possible with an empty stub library.
    let mut driver = Driver::new(typing_strict_config());
    driver.set_package("stub-ok", 0, "size <- nchar(\"hello\")");
    // `nchar : fn(character) -> integer`, so passing an integer mismatches.
    driver.set_package("stub-bad", 0, "size <- nchar(1L)");
    driver.set_package("stub-value", 0, "half <- pi");
}

#[test]
fn reexport_chain_and_cycle() {
    let mut driver = Driver::new(typing_config());
    // A monotone re-export chain a <- b <- c, with c a typed function; the chain converges to its scheme.
    driver.set_package("c", 2, "#: fn(x: integer) -> integer\nc_fn <- function(x) x + x");
    driver.set_package("b", 1, "b_fn <- c_fn");
    driver.set_package("a", 0, "a_fn <- b_fn");
    driver.set_package("use-chain", 3, "result <- a_fn(2L)");
    // A wrong-typed use of the re-exported chain errors.
    driver.set_package("use-chain-bad", 3, "result <- a_fn(\"two\")");
    // A genuine period-2 re-export cycle: both members pin to Unknown (no type error, no panic).
    driver.set_package("cycle-a", 0, "a_fn <- b_fn");
    driver.set_package("cycle-b", 1, "b_fn <- a_fn");
}

#[test]
fn interface_change_creates_and_resolves_dependent_error() {
    let mut driver = Driver::new(typing_config());
    driver.set_package(
        "v0-fn",
        0,
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count",
    );
    driver.set_package("v0-use", 1, "result <- double_count(2L)");
    // Change a.R so `double_count` now expects character: the referrer's clean call becomes an error,
    // purely from the interface change (the referrer's own text is untouched).
    driver.set_package(
        "interface-change",
        0,
        "#: fn(count: character) -> integer\ncount_letters <- function(count) 1L\ndouble_count <- count_letters",
    );
    // Revert the interface; the dependent error disappears.
    driver.set_package(
        "interface-revert",
        0,
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count",
    );
}

#[test]
fn add_delete_readd_same_path() {
    let mut driver = Driver::new(typing_config());
    driver.set_package("def", 0, "#: fn(x: integer) -> integer\nf <- function(x) x + x");
    driver.set_package("use", 1, "result <- f(2L)");
    // Delete the defining file: the referrer can no longer resolve `f`'s scheme.
    driver.delete("delete-def", 0);
    // Re-add the same slot (same path → tombstone re-add) with a different scheme that the call violates.
    driver.set_package("readd-def", 0, "#: fn(x: character) -> integer\nf <- function(x) 1L");
    // Re-add with the original scheme; the call is clean again.
    driver.set_package("readd-original", 0, "#: fn(x: integer) -> integer\nf <- function(x) x + x");
}

#[test]
fn package_script_reclassification() {
    let mut driver = Driver::new(typing_config());
    driver.set_package("def", 0, "#: fn(x: integer) -> integer\nf <- function(x) x + x");
    driver.set_package("ref-as-package", 1, "result <- f(\"bad\")");
    // Reclassify the referrer as a script: it no longer participates as a package document, but it still
    // references the package global `f`, so the same type error must hold.
    driver.set_script("ref-as-script", 1, "result <- f(\"bad\")");
    // Reclassify the *definer* as a script: `f` stops being a package global, so the reference dangles.
    driver.set_script("def-as-script", 0, "#: fn(x: integer) -> integer\nf <- function(x) x + x");
}

#[test]
fn rename_a_global_breaks_referrer() {
    let mut driver = Driver::new(typing_config());
    driver.set_package("def", 0, "#: fn(x: integer) -> integer\nhelper <- function(x) x + x");
    driver.set_package("use", 1, "result <- helper(2L)");
    // Rename the global: the referrer's name no longer resolves to a package global.
    driver.set_package("rename", 0, "#: fn(x: integer) -> integer\nhelper_renamed <- function(x) x + x");
    // Update the referrer to the new name and pass a wrong argument.
    driver.set_package("use-renamed-bad", 1, "result <- helper_renamed(\"two\")");
}

// ----------------------------------------------------------------------------------------------------
// Randomized, fixed-seed generator + adversarial edit stream
// ----------------------------------------------------------------------------------------------------

// The one characterized remaining engine gap, pinned as a deterministic repro.
//
// Two package files that reference each other's *values* (not re-exports) form a cycle in the
// `GlobalScheme` dependency graph. `ReexportScc` collapses only *re-export* cycles (`a <- b`; `b <- a`),
// so this value-reference cycle is not routed through the SCC fixed-point body. On a fresh build the
// `is_computing` guard breaks the back-edge to `Unknown` (matching production's bootstrap), so the *first*
// compute does not cycle. But once an edit makes both directions' last compute record the opposite edge,
// the *recorded* memo graph has `GlobalScheme(a) → GlobalScheme(b)` and `GlobalScheme(b) → GlobalScheme(a)`
// at once; validation then re-enters `recompute` on a key already on the stack and trips the core's
// accidental-cycle guard. Production has no such trouble: its package-interface fixed-point owns all
// globals in one body and iterates to convergence.
//
// Root cause: the general package-interface fixed-point (`DESIGN.md` §5, but for *arbitrary* mutual
// value references, not just re-exports) is not yet ported into the per-symbol layer. The fix is to
// generalize `ReexportScc`/`ReexportInterface` to an SCC over the full symbol-reference graph with an
// iterative inference body — its own slice. This test asserts the *current* behavior (a panic) so the day
// the fixed-point lands, it fails and is updated to assert parity.
#[test]
fn mutual_value_reference_cycle_is_the_documented_gap() {
    let result = std::panic::catch_unwind(|| {
        let mut engine = Engine::new(RoughlyQueries::new());
        let set = |engine: &mut Engine<RoughlyQueries>, files: &[(FileId, &str)]| {
            let ids = files.iter().map(|(id, _)| *id).collect::<Vec<_>>();
            engine.set_input(Key::ProjectFiles, ids);
            engine.set_input(Key::Config, typing_config());
            for (id, source) in files {
                engine.set_input(Key::SourceText(*id), (*source).to_owned());
                engine.set_input(Key::DocumentKind(*id), DocumentKind::Package);
            }
        };
        // Build with `beta` a leaf. `gamma <- alpha(...)` forces `GlobalScheme(alpha)` to be computed,
        // which records a (one-directional) recorded edge `GlobalScheme(alpha) → GlobalScheme(beta)`.
        let fetch_all = |engine: &Engine<RoughlyQueries>, ids: &[FileId]| {
            for id in ids {
                let _ = engine.fetch::<FileDiagnostics>(Key::Diagnostics(*id));
            }
        };
        set(
            &mut engine,
            &[
                (0, "alpha <- beta(1L)"),
                (1, "beta <- 1L"),
                (2, "gamma <- alpha(1L)"),
            ],
        );
        fetch_all(&engine, &[0, 1, 2]);
        // Edit `beta` to reference `alpha`, closing the value-reference cycle: now `GlobalScheme(beta)`
        // records `→ GlobalScheme(alpha)` while `alpha`'s memo still holds `→ GlobalScheme(beta)`.
        set(
            &mut engine,
            &[
                (0, "alpha <- beta(1L)"),
                (1, "beta <- alpha(1L)"),
                (2, "gamma <- alpha(1L)"),
            ],
        );
        fetch_all(&engine, &[0, 1, 2]);
    });
    assert!(
        result.is_err(),
        "EXPECTED the mutual value-reference cycle to still trip the cycle guard (the documented engine \
         gap). If this now succeeds, the general package-interface fixed-point has been implemented — \
         replace this test with a parity assertion."
    );
}

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> SplitMix64 {
        SplitMix64 { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }

    fn chance(&mut self, numerator: u64, denominator: u64) -> bool {
        self.below(denominator) < numerator
    }
}

// The shared global-name alphabet. Cross-file references draw from this pool, so a reference resolves or
// dangles as files defining the name come and go — generating and resolving cross-file errors organically.
const NAMES: [&str; 4] = ["alpha", "beta", "gamma", "delta"];

// Generate one file's source from a handful of items. Items deliberately produce the cross-file and
// stdlib interactions the deferrals target: typed function/value definitions, well- and mis-typed calls
// of pooled names and stdlib stubs, re-exports, an `@alias` definition and use, and plain bindings.
//
// Each file is given a random **rank floor** `r`: every pool name it *defines* has rank ≥ `r`, and every
// pool name it *references* has rank < `r` (rank = index in `NAMES`). Because `GlobalScheme(g)` infers
// `g`'s whole defining file — depending on *every* pool global that file references (co-resident bindings
// included, which is exactly what a per-*item* rank constraint missed) — this floor makes every edge of
// the package-global `GlobalScheme` graph strictly rank-decreasing, so it is a DAG: the stream never
// constructs a *mutual* value-reference cycle. That one input class is the characterized remaining engine
// gap (the general package-interface fixed-point for non-re-export cycles is unimplemented — only
// re-export cycles are handled, via `ReexportScc`/`ReexportInterface`); it is pinned separately by
// `mutual_value_reference_cycle_is_the_documented_gap`, not swept under the rug here.
fn generate_source(rng: &mut SplitMix64) -> String {
    let item_count = 1 + rng.below(3);
    let mut lines = Vec::new();
    // The file's rank floor: names it defines are ranked ≥ floor; names it references are ranked < floor.
    let floor = rng.below(NAMES.len() as u64) as usize;
    // Optionally declare the package alias `Count` (resolves cross-file uses of `#: Count`). Type
    // definitions are not part of the value `GlobalScheme` graph, so they are floor-independent.
    if rng.chance(1, 4) {
        lines.push("#: @alias Count {integer}".to_owned());
    }
    // A defined pool name (rank ≥ floor). Every file has at least `delta`, so this never underflows.
    let define = |rng: &mut SplitMix64| NAMES[floor + rng.below((NAMES.len() - floor) as u64) as usize];
    for _ in 0..item_count {
        let name = define(rng);
        // A pool reference is only possible when the floor leaves a strictly-lower-rank target available.
        let reference_target = if floor == 0 {
            None
        } else {
            Some(NAMES[rng.below(floor as u64) as usize])
        };
        // Choose an item kind; pool-referencing kinds are only used when a reference target exists, so a
        // floor-0 file degrades them to a plain definition (still varied via the stdlib/alias kinds).
        let kind = rng.below(9);
        let kind = if reference_target.is_none() && matches!(kind, 2 | 3 | 4) {
            8
        } else {
            kind
        };
        let target = reference_target.unwrap_or("alpha");
        match kind {
            0 => {
                lines.push("#: integer".to_owned());
                lines.push(format!("{name} <- 1L"));
            }
            1 => {
                lines.push("#: fn(x: integer) -> integer".to_owned());
                lines.push(format!("{name} <- function(x) x + x"));
            }
            2 => lines.push(format!("{name} <- {target}(1L)")),
            3 => lines.push(format!("{name} <- {target}(\"s\")")),
            4 => lines.push(format!("{name} <- {target}")),
            5 => lines.push(format!("{name} <- nchar(\"x\")")),
            6 => lines.push(format!("{name} <- nchar(1L)")),
            7 => {
                lines.push("#: Count".to_owned());
                lines.push(format!("{name} <- 1L"));
            }
            _ => lines.push(format!("{name} <- 1L")),
        }
    }
    lines.join("\n")
}

const MAX_SLOTS: FileId = 5;

#[test]
fn randomized_adversarial_edit_stream() {
    // Fixed seeds make any failure a deterministic repro; bump/scan seeds locally when hunting. Each seed
    // drives a long adversarial stream (every step rebuilds the oracle from scratch and asserts parity).
    for seed in [
        0x1234_5678u64,
        0xDEAD_BEEF,
        0x0F0F_0F0F,
        42,
        0xA5A5_A5A5,
        0xCAFE_F00D,
        7,
        0xFFFF_0001,
    ] {
        run_randomized_stream(seed, 300);
    }
}

fn run_randomized_stream(seed: u64, steps: usize) {
    let mut rng = SplitMix64::new(seed);
    let mut driver = Driver::new(typing_strict_config());

    for step in 0..steps {
        let label = format!("seed={seed:#x} step={step}");
        let slot = rng.below(MAX_SLOTS as u64) as FileId;
        let present = driver.workspace.files.contains_key(&slot);

        match rng.below(10) {
            // Toggle strict mode (a config edit exercised mid-stream).
            0 => {
                let strict = !driver.workspace.config.strict;
                driver.set_strict(&label, strict);
            }
            // Delete an existing slot, else add one.
            1 if present => driver.delete(&label, slot),
            // Reclassify an existing slot, keeping its source.
            2 if present => {
                let source = driver.workspace.files[&slot].source.clone();
                let package = !driver.workspace.files[&slot].package;
                driver.step(&label, |workspace| {
                    workspace.files.insert(slot, FileState { source, package });
                });
            }
            // Otherwise (add a new slot, or edit an existing one) regenerate its source. Most slots are
            // package files; some are scripts.
            _ => {
                let source = generate_source(&mut rng);
                let package = if present {
                    driver.workspace.files[&slot].package
                } else {
                    rng.chance(3, 4)
                };
                driver.step(&label, |workspace| {
                    workspace.files.insert(slot, FileState { source, package });
                });
            }
        }
    }
}
