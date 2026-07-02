//! The differential correctness check (`DESIGN.md` §7).
//!
//! # What this proves
//!
//! The engine, driven *incrementally* through an edit stream, produces the same diagnostics as the
//! `analysis` crate's **full from-scratch rebuild** of the *final* state after every edit. The oracle is
//! deliberately `analysis`'s `run_full` on a freshly built `Analysis` for the current file set — never
//! `analysis`'s own incremental path — because comparing against an incremental path could ratify a stale
//! result on both sides. `analysis`'s debug drift assertions run inside `run_full`, so the oracle is
//! self-checked.
//!
//! # Comparison scope
//!
//! After each edit, for every current file, the engine's `Diagnostics(f)` must equal the oracle's
//! `document_diagnostics(f)` as a **normalized set** (sorted by range, then code, then severity, then
//! message). The two engines use independent interners, so the comparison is over *rendered* facts (byte
//! range + code + severity + message), which are interner-independent.
//!
//! We compare **every diagnostic class production emits** with **zero exclusions** — local naming, package
//! naming, type errors, strict mode, lowering (syntax) errors, and lint (`DiagnosticCode::Naming`,
//! `SyntaxError`, `TypeError`, `Strict`, and `Lint`):
//!
//! - **Lowering diagnostics** (the lowering-originated `SyntaxError`s, and `AnnotationError`): the engine's
//!   `LoweringDiagnostics(f)` query calls the now-public `analysis::lower::lower_with_diagnostics` — the
//!   *same* function production lowers through — which short-circuits to `collect_syntax_errors` on a
//!   malformed tree and otherwise returns the lowering-pass diagnostics, so they are byte-identical on both
//!   sides. (`AnnotationError` does not arise anyway — its constructors are dead in `analysis` — but the
//!   class is compared rather than excluded, so if it ever did, both sides would have to agree.)
//! - **Lint** (`DiagnosticCode::Lint`): the engine's `Lint(f)` query calls the *same* `analysis::lint::analyze`
//!   with the same `[lint]` config, so lint diagnostics are identical by construction.
//!
//! **Package naming** (`package_document_diagnostics` — could-not-resolve, builtin shadow, overwrite pairs,
//! duplicate-type, and type-reference) is `analysis`'s cross-file subsystem, `pub(crate)` and unreachable
//! across the crate boundary; the engine **re-derives** it as fine-grained queries (`PackageNamingDiagnostics(f)`
//! over `DefiningItem`/`DefinerOrder`/`TypeNameStatus` firewalls), reproducing production's wording and ranges
//! byte-for-byte. **Local naming** is the *same* `resolve_document_locally` verbatim, identical by construction.
//!
//! So there is no exclusion to mask a divergence: any diagnostic in any class the engine produces that the
//! oracle does not (or vice versa) fails the normalized-set assertion immediately, on every file.
//!
//! # Drivers
//!
//! 1. A curated set of deterministic scenarios mirroring the `analysis` cross-file fixtures (cross-file
//!    function/value use and mismatch, cross-file `@alias`/`@type`, stdlib base-name use, re-export chains
//!    and a period-2 cycle, an interface-change-creates-error incremental edit, add→delete→re-add of the
//!    same path, package↔script reclassification, and a global rename), plus a dedicated package-naming
//!    scenario exercising every `package_document_diagnostics` category (the builtin/stub shadow, the
//!    type-reference arity / `@new`-on-alias sub-cases, and the script-local type overlay the generator
//!    never emits).
//! 2. A randomized, fixed-seed generator over a small realistic R alphabet producing multi-file
//!    package+script workspaces and adversarial edit streams (edit→fetch→edit, add/delete/re-add the same
//!    slot, reclassification, cross-file errors introduced and resolved, strict toggling). Parity is
//!    asserted after **every** step; the seed is fixed so a failure is a deterministic repro.

use {
    analysis::{
        Analysis, CheckConfig, Diagnostic, DiagnosticCode, LintConfig, Severity,
        document::Document, naming::DocumentKind, run_full, tree::new_parser,
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
    analysis.document_diagnostics(document_id)
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

fn engine_diagnostics(
    engine: &Engine<RoughlyQueries>,
    id: FileId,
    config: &Config,
) -> Vec<Diagnostic> {
    let file_diagnostics = engine.fetch::<FileDiagnostics>(Key::Diagnostics(id));
    let fallback = *engine.fetch::<Range>(Key::FallbackRange);
    let mut rendered = Vec::new();
    // Local-naming and package-naming diagnostics are not config-gated in production's
    // `document_diagnostics` (they come from `resolve_package`, which `typecheck` always runs), so they
    // are emitted unconditionally here too. `naming` is the verbatim `resolve_document_locally` output;
    // `package_naming` reproduces `package_document_diagnostics`.
    rendered.extend(file_diagnostics.naming.iter().cloned());
    rendered.extend(file_diagnostics.package_naming.iter().cloned());
    // Lowering (syntax) and lint diagnostics are emitted unconditionally in production's
    // `document_diagnostics` (not config-gated), so they are rendered here the same way.
    rendered.extend(file_diagnostics.lowering.iter().cloned());
    rendered.extend(file_diagnostics.lint.iter().cloned());
    // Unused-local warnings (`DiagnosticCode::Naming`) are config-gated in production's
    // `document_diagnostics` (`check_config.unused`), so the engine computes them unconditionally and the
    // consumer gates them here — the same shape as the type/strict classes below.
    if config.unused {
        rendered.extend(file_diagnostics.unused.iter().cloned());
    }
    if config.typing {
        // Render the raw inference errors against the engine's own interner + fallback range, exactly as
        // production's `typecheck` renders them.
        engine.group().with_interner(|interner| {
            for error in &file_diagnostics.type_errors {
                rendered.push(Diagnostic::from_inference_error(error, fallback, interner));
            }
        });
    }
    // Mirrors production's per-file strict gate (`strict_override` wins over the config), including
    // the escalation of unresolved references to errors under strict.
    if file_diagnostics.strict_override.unwrap_or(config.strict) {
        rendered.extend(file_diagnostics.strict_diagnostics.iter().cloned());
        for diagnostic in &mut rendered {
            diagnostic.escalate_unresolved_to_error();
        }
    }
    // Mirrors production's suppression-comment filter at assembly.
    let source = engine.fetch::<String>(Key::SourceText(id));
    analysis::diagnostic::apply_suppressions(rendered, &source)
}

// ----------------------------------------------------------------------------------------------------
// Normalization + the parity assertion
// ----------------------------------------------------------------------------------------------------

type NormalizedDiagnostic = (usize, usize, u8, u8, String);

fn code_rank(code: DiagnosticCode) -> u8 {
    match code {
        DiagnosticCode::Lint(_) => 0,
        DiagnosticCode::Naming => 1,
        DiagnosticCode::SyntaxError => 2,
        DiagnosticCode::TypeError => 3,
        DiagnosticCode::AnnotationError => 4,
        DiagnosticCode::Strict => 5,
        DiagnosticCode::Unused => 6,
        DiagnosticCode::Unresolved => 7,
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
        // On malformed input both engines lower to an *empty* module (production's `lower_with_diagnostics`
        // short-circuit and the engine's `Lower`), so every downstream class is empty; the only diagnostics
        // are the lowering *syntax* errors (`collect_syntax_errors`), which the engine now reproduces via its
        // `LoweringDiagnostics` query through the *same* `lower_with_diagnostics`. So a malformed file reaches
        // full parity with no per-file exclusion — every diagnostic class is compared on every file.
        let engine_set = normalize(engine_diagnostics(engine, *id, &workspace.config));
        let oracle_set = normalize(oracle_diagnostics(&oracle, *id, state.package));
        assert_eq!(
            engine_set,
            oracle_set,
            "parity divergence at step `{label}` for file {id} (path {:?})\n\
             engine: {engine_set:#?}\noracle: {oracle_set:#?}\nsource:\n{}",
            absolute_path(*id, state.package),
            state.source,
        );
    }
}

// Whether a source is malformed at the tree-sitter level (`root.has_error()`). Such a file lowers to an
// empty module on *both* sides, so its only divergence is production's lowering syntax diagnostics, which
// `assert_parity` excludes per malformed file. Re-parsing here (rather than reading engine state) keeps the
// discriminator independent: an empty module from an empty-but-well-formed source is *not* treated this way.
fn is_malformed(source: &str) -> bool {
    let mut parser = new_parser().expect("harness: R grammar should load");
    let document = Document::parse(&mut parser, source).expect("harness: source should parse");
    document.tree().root_node().has_error()
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
        lint: LintConfig::default(),
    }
}

fn typing_strict_config() -> Config {
    Config {
        typing: true,
        strict: true,
        unused: false,
        lint: LintConfig::default(),
    }
}

// Everything on, including the `unused` check. The randomized stream uses this so the unused-local class is
// asserted continuously (the generator emits no unused locals, so the engine must likewise emit none — a
// guard that the gate never fires spuriously), and the curated `unused` scenario uses the typing+unused
// variant for the reported-warning cases.
fn typing_strict_unused_config() -> Config {
    Config {
        typing: true,
        strict: true,
        unused: true,
        lint: LintConfig::default(),
    }
}

fn typing_unused_config() -> Config {
    Config {
        typing: true,
        strict: false,
        unused: true,
        lint: LintConfig::default(),
    }
}

// Typing and strict both off: production's `run_full` takes the `resolve_package`-only branch (it does not
// call `typecheck`), yet still publishes package-naming diagnostics. Used to assert package-naming parity
// in that distinct oracle code path, since the randomized stream always keeps typing on.
fn naming_only_config() -> Config {
    Config {
        typing: false,
        strict: false,
        unused: false,
        lint: LintConfig::default(),
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
    driver.set_package(
        "structural-rejected",
        1,
        "result <- get_name(list(name = \"bob\"))",
    );
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
    driver.set_package(
        "c",
        2,
        "#: fn(x: integer) -> integer\nc_fn <- function(x) x + x",
    );
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
    driver.set_package(
        "def",
        0,
        "#: fn(x: integer) -> integer\nf <- function(x) x + x",
    );
    driver.set_package("use", 1, "result <- f(2L)");
    // Delete the defining file: the referrer can no longer resolve `f`'s scheme.
    driver.delete("delete-def", 0);
    // Re-add the same slot (same path → tombstone re-add) with a different scheme that the call violates.
    driver.set_package(
        "readd-def",
        0,
        "#: fn(x: character) -> integer\nf <- function(x) 1L",
    );
    // Re-add with the original scheme; the call is clean again.
    driver.set_package(
        "readd-original",
        0,
        "#: fn(x: integer) -> integer\nf <- function(x) x + x",
    );
}

#[test]
fn package_script_reclassification() {
    let mut driver = Driver::new(typing_config());
    driver.set_package(
        "def",
        0,
        "#: fn(x: integer) -> integer\nf <- function(x) x + x",
    );
    driver.set_package("ref-as-package", 1, "result <- f(\"bad\")");
    // Reclassify the referrer as a script: it no longer participates as a package document, but it still
    // references the package global `f`, so the same type error must hold.
    driver.set_script("ref-as-script", 1, "result <- f(\"bad\")");
    // Reclassify the *definer* as a script: `f` stops being a package global, so the reference dangles.
    driver.set_script(
        "def-as-script",
        0,
        "#: fn(x: integer) -> integer\nf <- function(x) x + x",
    );
}

#[test]
fn rename_a_global_breaks_referrer() {
    let mut driver = Driver::new(typing_config());
    driver.set_package(
        "def",
        0,
        "#: fn(x: integer) -> integer\nhelper <- function(x) x + x",
    );
    driver.set_package("use", 1, "result <- helper(2L)");
    // Rename the global: the referrer's name no longer resolves to a package global.
    driver.set_package(
        "rename",
        0,
        "#: fn(x: integer) -> integer\nhelper_renamed <- function(x) x + x",
    );
    // Update the referrer to the new name and pass a wrong argument.
    driver.set_package("use-renamed-bad", 1, "result <- helper_renamed(\"two\")");
}

// Curated package-naming regression scenarios — one per `package_document_diagnostics` category, plus the
// type-reference sub-cases (arity, `@new`-on-alias) and the script-local type overlay that the randomized
// generator never emits. The builtin/stub-shadow category in particular is exercised nowhere else: the
// generator draws names from a pool disjoint from the builtins and stubs. Parity is asserted after each
// step, so every category must match the full-rebuild oracle exactly — wording, range, severity, and code.
#[test]
fn package_naming_diagnostics_match_production() {
    let mut driver = Driver::new(typing_config());

    // (D) could-not-resolve: a reference to a value name no package file defines.
    driver.set_package("could-not-resolve", 0, "result <- mystery(1L)");

    // (B) builtin/stub shadow: top-level bindings named like a stdlib stub (`pi`) and a hardcoded
    // builtin (`c`) — one per shadow branch.
    driver.set_package("shadow-builtin-and-stub", 1, "pi <- 3L\nc <- 1L");

    // (A) overwrite pair across files: file 2 (path-earlier) and file 3 (path-later) both define `shared`.
    driver.set_package("overwrite-earlier", 2, "shared <- 1L");
    driver.set_package("overwrite-later", 3, "shared <- 2L");

    // (A) within-file overwrite: two top-level assignments to the same name in one file.
    driver.set_package("overwrite-within-file", 4, "dup <- 1L\ndup <- 2L");

    // (T) duplicate-type + (C) reference-to-duplicate: `Count` declared in two package files becomes a
    // duplicate (each declaration site flagged) and stays unresolved at its references.
    driver.set_package("dup-type-first", 5, "#: @alias Count {integer}");
    driver.set_package(
        "dup-type-second",
        6,
        "#: @alias Count {integer}\n\n#: Count\nlabel <- 1L",
    );

    // (C) unknown type: a reference to a type no package file defines.
    driver.set_package("unknown-type", 7, "#: Mystery\nvalue <- 1L");

    // (C) arity mismatch: a generic `@type` referenced with the wrong number of type arguments.
    driver.set_package(
        "arity-mismatch",
        8,
        "#: @type Box<T> {list{value: T}}\n\n#: Box\nboxed <- 1L",
    );

    // (C) `@new` on an alias: `@new` requires a nominal `@type`, not an `@alias`.
    driver.set_package(
        "new-on-alias",
        9,
        "#: @alias Tag {integer}\n\n#: @new Tag\ntagged <- 1L",
    );

    // A script contributes only (C) and (D); its own `@type` shadows package ones, so `@new Local`
    // resolves locally and raises no diagnostic — guarding the script-local type overlay against a
    // spurious unknown-type error.
    driver.set_script(
        "script-local-type",
        10,
        "#: @type Local {list{x: integer}}\n\n#: @new Local\nlocal_value <- list(x = 1L)",
    );

    // Resolve the cross-file duplicate so the duplicate-type and reference-to-duplicate diagnostics clear
    // together (the type-name reverse-dependency hop), proving they appear and disappear at parity.
    driver.delete("dup-type-resolve", 5);
}

// Package-naming parity in the typing-off oracle path: with typing and strict both off, production's
// `run_full` resolves the package without typechecking, so this asserts the package-naming diagnostics are
// published (and match) independently of the type/strict gates — the path the randomized stream never takes.
#[test]
fn package_naming_diagnostics_match_production_without_typing() {
    let mut driver = Driver::new(naming_only_config());
    driver.set_package("shadow", 0, "pi <- 3L\nc <- 1L");
    driver.set_package("could-not-resolve", 1, "result <- mystery(1L)");
    driver.set_package("overwrite-earlier", 2, "shared <- 1L");
    driver.set_package("overwrite-later", 3, "shared <- 2L");
    driver.set_package("dup-type-first", 4, "#: @alias Count {integer}");
    driver.set_package(
        "dup-type-and-ref",
        5,
        "#: @alias Count {integer}\n\n#: Count\nlabel <- 1L",
    );
    driver.set_package(
        "new-on-alias",
        6,
        "#: @alias Tag {integer}\n\n#: @new Tag\ntagged <- 1L",
    );
}

// GAP #3: malformed input. A tree-sitter parse error lowers to an empty module on both sides (production's
// `lower_with_diagnostics` short-circuit and the engine's `Lower`), so the engine emits no downstream
// diagnostics off the partial error-recovery tree — matching production, whose only residual output is the
// lowering syntax diagnostics the harness excludes per malformed file. This first asserts every malformed
// template is genuinely `has_error()`, then drives a malformed-edit cycle: making a definer malformed drops
// its global package-wide, so a referrer's cross-file diagnostics shift identically on both sides.
#[test]
fn malformed_sources_are_detected_and_reach_parity() {
    for source in MALFORMED_SOURCES {
        assert!(
            is_malformed(source),
            "template must be tree-sitter malformed (has_error): {source:?}"
        );
    }

    let mut driver = Driver::new(typing_strict_config());
    driver.set_package(
        "def",
        0,
        "#: fn(x: integer) -> integer\nhelper <- function(x) x + x",
    );
    driver.set_package("use", 1, "result <- helper(2L)");
    // Make the definer malformed (incomplete `function` head): `helper` disappears from the package, so the
    // referrer can no longer resolve it — and the definer itself emits only its lowering syntax errors, which
    // the engine now reproduces (via `LoweringDiagnostics`) and the harness compares at full parity.
    driver.set_package("def-malformed", 0, "helper <- function(x");
    // Repair it: parity returns to the clean cross-file state.
    driver.set_package(
        "def-repaired",
        0,
        "#: fn(x: integer) -> integer\nhelper <- function(x) x + x",
    );
}

// A *well-formed* tree (no `root.has_error()`) whose lowering pass rejects a directive mix ("cannot mix
// definition and annotation directives") — the other branch of `lower_with_diagnostics`, where production
// lowers and returns the lowering-pass diagnostics rather than `collect_syntax_errors`. The engine's
// `LoweringDiagnostics` runs the same function, so this lowering `SyntaxError` matches at full parity; it is
// the curated coverage of the well-formed lowering-error path the randomized generator deliberately avoids.
#[test]
fn directive_mix_lowering_error_matches_production() {
    // `@alias Count` (a *definition* directive) merged with `#: integer` (an *annotation* directive) into one
    // `#:` block, which lowering rejects. Non-vacuity: assert the oracle really emits the lowering SyntaxError.
    let source = "#: @alias Count {integer}\n#: integer\nvalue <- 1L";
    let mut workspace = Workspace::new(typing_strict_config());
    workspace.files.insert(
        0,
        FileState {
            source: source.to_owned(),
            package: true,
        },
    );
    let oracle = build_oracle(&workspace);
    assert!(
        oracle_diagnostics(&oracle, 0, true)
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::SyntaxError),
        "the directive-mix scenario must produce a lowering SyntaxError on the oracle"
    );

    let mut driver = Driver::new(typing_strict_config());
    driver.set_package("directive-mix", 0, source);
    // Separating the directives with a blank line clears the lowering error; parity holds across the fix.
    driver.set_package(
        "directive-mix-fixed",
        0,
        "#: @alias Count {integer}\n\n#: integer\nvalue <- 1L",
    );
}

// Unused-assignment warnings (`DiagnosticCode::Unused`, "`{name}` is assigned but never used."),
// synthesized from `NamesLocal::unused_assignments` and gated by the `unused` check. The randomized
// stream emits no unused assignments, so this curated `unused: true` pass is the only coverage of the
// reported case. It exercises the reaching-write eligibility rules from `resolve_document_locally`: a
// reported dead local, a used local, a `.`-prefixed throwaway, a parameter, a `for` variable, a
// top-level binding, a conditional update and a loop accumulator that later reads keep alive, and a
// dead store killed by a rewrite. The final step toggles the check off, asserting the warnings clear
// at parity.
#[test]
fn unused_local_warnings_match_production() {
    let mut driver = Driver::new(typing_unused_config());
    // `unused_value` is a function-local assignment never read -> reported; `used_value` is read -> not.
    driver.set_package(
        "unused-and-used-locals",
        0,
        "f <- function() {\n  unused_value <- 1L\n  used_value <- 2L\n  used_value\n}",
    );
    // A `.`-prefixed local is an intentional throwaway -> never reported.
    driver.set_package(
        "throwaway-local",
        1,
        "g <- function() {\n  .ignored <- 1L\n  used <- 2L\n  used\n}",
    );
    // Parameters, `for` variables, and top-level bindings are never reported even when unused.
    driver.set_package(
        "params-for-and-top-level",
        2,
        "top_level <- 1L\nh <- function(unused_param) {\n  for (item in 1:3) 1L\n}",
    );
    // A conditional update and a loop accumulator both reach a later read -> not reported; the
    // pre-rewrite dead store is -> reported. Also exercises the loop/branch join paths under typing.
    driver.set_package(
        "reaching-writes",
        3,
        "k <- function(flag) {\n  x <- 1L\n  if (flag) {\n    x <- 2L\n  }\n  total <- 0L\n  for (i in 1:3) {\n    total <- total + i\n  }\n  dead <- 1L\n  dead <- 2L\n  x + total + dead\n}",
    );
    // Toggling the check off clears the warnings on both sides (the unused-off oracle path).
    driver.step("unused-off", |workspace| workspace.config.unused = false);
}

// A NON-EMPTY strict-origin case. The randomized stream and the other curated cases never force a genuine
// strict `Unknown` origin (so the strict-origin rendering sub-path went unverified vs production); this
// closes that gap. `1L %then% 2L` is a custom infix operator the checker does not model, so it lowers to an
// `Unsupported` expression whose type is an undetermined `Unknown` — a real O1 strict origin. The binding
// form flags the name. Asserts the oracle actually emits it (non-vacuity), drives it through the parity
// harness, then clears it with `@trust Any` (which strict tolerates), proving appearance and disappearance
// both hold at parity.
#[test]
fn strict_origin_non_empty_matches_production() {
    let mut workspace = Workspace::new(typing_strict_config());
    workspace.files.insert(
        0,
        FileState {
            source: "x <- 1L %then% 2L".to_owned(),
            package: true,
        },
    );
    let oracle = build_oracle(&workspace);
    assert!(
        oracle_diagnostics(&oracle, 0, true)
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::Strict),
        "the strict-origin scenario must produce a Strict diagnostic on the oracle (non-vacuity)"
    );

    let mut driver = Driver::new(typing_strict_config());
    driver.set_package("strict-origin", 0, "x <- 1L %then% 2L");
    // `@trust Any` makes the binding `Any`, which strict tolerates, so the origin clears.
    driver.set_package(
        "strict-origin-cleared",
        0,
        "#: @trust Any\nx <- 1L %then% 2L",
    );
}

// ----------------------------------------------------------------------------------------------------
// Randomized, fixed-seed generator + adversarial edit stream
// ----------------------------------------------------------------------------------------------------

// Mutual cross-file *value* references — the cycle class that used to be the documented engine gap, now
// resolved by the general interface SCC fixed-point and asserted at full parity with production.
//
// Two package files that reference each other's *values* (not re-exports) form a cycle in the
// `GlobalScheme` dependency graph: `GlobalScheme(alpha)` infers file 0, which reads `GlobalScheme(beta)`,
// which infers file 1, which reads `GlobalScheme(alpha)` again. `SymbolScc` detects this cycle over the
// general `InterfaceDeps` graph (not just re-exports) and routes both members through one `InterfaceScc`
// fixed-point that bootstraps to `Unknown` and iterates to convergence — exactly as production's
// package-interface loop owns all globals in one body. Previously the *recorded* memo graph closed the
// back-edge and tripped the core's accidental-cycle guard; now no in-SCC `GlobalScheme` key re-enters
// itself, so the guard stays reserved for genuine engine bugs. This drives the same build→edit sequence
// through the parity harness, asserting the engine matches the full-rebuild oracle at every step.
#[test]
fn mutual_value_reference_cycle_matches_production() {
    let mut driver = Driver::new(typing_config());
    // Build with `beta` a leaf; `gamma <- alpha(...)` forces `GlobalScheme(alpha)`, recording the
    // one-directional edge `GlobalScheme(alpha) → GlobalScheme(beta)`.
    driver.set_package("alpha-calls-beta", 0, "alpha <- beta(1L)");
    driver.set_package("beta-leaf", 1, "beta <- 1L");
    driver.set_package("gamma-calls-alpha", 2, "gamma <- alpha(1L)");
    // Edit `beta` to reference `alpha`, closing the value-reference cycle (alpha ↔ beta). The engine now
    // routes both through the interface SCC fixed-point instead of re-entering a key on the compute stack.
    driver.set_package("beta-calls-alpha", 1, "beta <- alpha(1L)");
    // Re-open the cycle: `beta` becomes a leaf again, so the SCC dissolves back to two trivial nodes.
    driver.set_package("beta-leaf-again", 1, "beta <- 2L");
}

// A pure un-annotated 2-cycle of mutual value references converges to `Unknown` on both sides (no concrete
// base), so a downstream consumer sees no concrete scheme — matching production exactly.
#[test]
fn unannotated_two_cycle_converges_to_unknown() {
    let mut driver = Driver::new(typing_strict_config());
    driver.set_package("a", 0, "a <- function() b()");
    driver.set_package("b", 1, "b <- function() a()");
    driver.set_package("use", 2, "result <- a()");
}

// A 3-cycle of mutual value references (a → b → c → a) likewise converges; the SCC fixed-point owns all
// three members in one body.
#[test]
fn unannotated_three_cycle_converges() {
    let mut driver = Driver::new(typing_strict_config());
    driver.set_package("a", 0, "a <- function() b()");
    driver.set_package("b", 1, "b <- function() c()");
    driver.set_package("c", 2, "c <- function() a()");
    driver.set_package("use", 3, "result <- a()");
}

// An annotated member breaks the cycle: `a` carries an explicit signature, so its exported scheme is its
// annotation independent of the others, and the rest of the cycle resolves against it. Both the annotated
// and unannotated members must match production.
#[test]
fn annotated_member_breaks_the_cycle() {
    let mut driver = Driver::new(typing_config());
    driver.set_package("a-annotated", 0, "#: fn() -> integer\na <- function() b()");
    driver.set_package("b", 1, "b <- function() a()");
    // A consumer of the annotated member calls it well- then ill-typed; the annotation drives both checks.
    driver.set_package("use-ok", 2, "result <- a()");
    driver.set_package("use-bad", 2, "#: character\nresult <- a()");
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

// Structurally malformed R — each makes `root.has_error()` true: unbalanced brackets, dangling operators,
// and incomplete `function`/`if`/block heads, the canonical live-editing intermediate states. The engine
// reproduces their lowering syntax diagnostics (`collect_syntax_errors`) via `LoweringDiagnostics`, so they
// are compared at full parity like every other class. The *well-formed*-tree lowering error (directive mix)
// is exercised separately by `directive_mix_lowering_error_matches_production`.
// `malformed_sources_are_detected_and_reach_parity` asserts every entry here is actually malformed.
const MALFORMED_SOURCES: [&str; 6] = [
    "alpha <-",
    "beta <- (1L +",
    "gamma <- function(x",
    "if (alpha",
    "{ delta <- 1L",
    "result <- alpha(1L",
];

// Generate one file's source from a handful of items. Items deliberately produce the cross-file and
// stdlib interactions the deferrals target: typed function/value definitions, well- and mis-typed calls
// of pooled names and stdlib stubs, re-exports, an `@alias` definition and use, and plain bindings.
//
// References draw *freely* from the whole name pool, with no rank ordering — so the package-global
// `GlobalScheme` graph is an arbitrary directed graph that routinely contains mutual value-reference
// cycles (`a` references `b` while `b` references `a`, across files, neither a re-export). These are
// exactly the cycles the general interface SCC fixed-point (`SymbolScc`/`InterfaceScc`) resolves; the
// stream asserts full parity with the production package-interface loop over that cyclic space, so the
// fixed-point's convergence (un-annotated cycle → `Unknown`, annotated member → breaks the cycle) is
// exercised continuously rather than only by the curated cases above.
fn generate_source(rng: &mut SplitMix64) -> String {
    // Occasionally emit a structurally *malformed* file (a tree-sitter parse error). On most keystrokes a
    // live buffer is transiently malformed; production lowers such input to an empty module (the
    // `lower_with_diagnostics` short-circuit) and emits only its `collect_syntax_errors`, both of which the
    // engine reproduces (`Lower`'s short-circuit + `LoweringDiagnostics`), so parity — now including the
    // syntax-error class — holds with no per-file exclusion. A malformed file also drops its definitions
    // package-wide, so its referrers' cross-file diagnostics shift identically on both sides.
    if rng.chance(1, 7) {
        return MALFORMED_SOURCES[rng.below(MALFORMED_SOURCES.len() as u64) as usize].to_owned();
    }
    let item_count = 1 + rng.below(3);
    let mut lines = Vec::new();
    // Optionally declare the package alias `Count` (resolves cross-file uses of `#: Count`). The blank
    // line keeps this `@alias` *definition* directive from merging with a following `#:` annotation into one
    // block, which lowering rejects ("cannot mix definition and annotation directives") — a lowering-phase
    // `SyntaxError` the engine now does reproduce, but the randomized stream keeps `Count` well-formed so its
    // focus stays on the cross-file / type-cycle space; the directive-mix error is covered by the curated
    // `directive_mix_lowering_error_matches_production` case instead.
    if rng.chance(1, 4) {
        lines.push("#: @alias Count {integer}".to_owned());
        lines.push(String::new());
    }
    let define = |rng: &mut SplitMix64| NAMES[rng.below(NAMES.len() as u64) as usize];
    for _ in 0..item_count {
        let name = define(rng);
        // A reference target drawn freely from the pool — any name, including one that references back into
        // this file's definitions, which is how the stream constructs mutual cross-file value cycles.
        let target = NAMES[rng.below(NAMES.len() as u64) as usize];
        let kind = rng.below(9);
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
    let mut driver = Driver::new(typing_strict_unused_config());

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
