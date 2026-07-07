//! The IDE differential check: the engine-backed [`EngineIde`] view must
//! produce, for **every** cursor position over curated single- and cross-file workspaces, byte-identical
//! IDE output to the frozen `analysis` oracle (`analysis::ide::*` on a fresh `Analysis`). The oracle drives
//! its own phases; the engine serves the same facts off its memoized query graph through the shared
//! `analysis::ide::generic::*` orchestration. So this proves the two paths agree on the *identical* logic —
//! any divergence is a fact-provider bug, surfaced at the exact position and feature.
//!
//! The sweep covers all seven interactive features (hover, inlay hints, signature help, completion,
//! definition, references with and without the declaration, and rename) at every byte position of every
//! file, so cross-file resolution (a referrer's goto/hover landing in the defining file, a global's
//! references spanning files, a project-wide rename) is exercised exhaustively rather than at hand-picked
//! points. Location-bearing results are compared as a sorted set (LSP location order is not semantic, the
//! same normalization the diagnostic differential uses); the rendered results (hover/completion/signature)
//! are compared verbatim.

use {
    analysis::{
        Analysis, CheckConfig, LintConfig,
        ide::{self, Location, RenameResult},
        text::TextPosition,
    },
    engine::{
        Engine,
        ide_view::{EngineIde, PathTable},
        queries::{Config, FileId, Key, RoughlyQueries, parse_source_input},
    },
    std::{collections::BTreeMap, path::PathBuf},
};

// ----------------------------------------------------------------------------------------------------
// Workspace model (shared shape with test_differential)
// ----------------------------------------------------------------------------------------------------

#[derive(Clone)]
struct FileState {
    source: String,
    package: bool,
}

#[derive(Clone)]
struct Workspace {
    files: BTreeMap<FileId, FileState>,
    config: Config,
}

const BASE: &str = "/ws";

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

fn project_files_order(workspace: &Workspace) -> Vec<FileId> {
    let mut entries = workspace
        .files
        .iter()
        .map(|(id, state)| (relative_key(*id, state.package), *id))
        .collect::<Vec<_>>();
    entries.sort();
    entries.into_iter().map(|(_, id)| id).collect()
}

fn check_config(config: &Config) -> CheckConfig {
    CheckConfig {
        unused: config.check.unused,
        typing: config.check.typing,
        strict: config.check.strict,
    }
}

// ----------------------------------------------------------------------------------------------------
// The two systems under test
// ----------------------------------------------------------------------------------------------------

// A fresh `Analysis` for the current state — the frozen oracle. Its `ide::*` wrappers drive lower /
// resolve_package / typecheck themselves, so nothing is pre-run here.
fn build_oracle(workspace: &Workspace) -> Analysis {
    let mut analysis = Analysis::new(
        PathBuf::from(BASE),
        LintConfig::default(),
        check_config(&workspace.config),
    );
    for id in project_files_order(workspace) {
        let state = &workspace.files[&id];
        analysis
            .add_document_from_source(absolute_path(id, state.package), &state.source)
            .expect("oracle: source should parse");
    }
    analysis
}

// One engine populated from the current state, with the host `PathTable` kept in lockstep with the
// `ProjectFiles` input exactly as the production host will keep it.
fn build_engine(workspace: &Workspace) -> (Engine<RoughlyQueries>, PathTable) {
    let mut engine = Engine::new(RoughlyQueries::new());
    let mut paths = PathTable::new(PathBuf::from(BASE));
    for (id, state) in &workspace.files {
        engine.set_input(Key::SourceText(*id), parse_source_input(&state.source));
        engine.set_input(Key::FileName(*id), absolute_path(*id, state.package));
        engine.set_input(
            Key::DocumentKind(*id),
            if state.package {
                analysis::naming::DocumentKind::Package
            } else {
                analysis::naming::DocumentKind::Script
            },
        );
        paths.insert(*id, absolute_path(*id, state.package));
    }
    engine.set_input(Key::ProjectFiles, project_files_order(workspace));
    engine.set_input(Key::Config, workspace.config.clone());
    (engine, paths)
}

// ----------------------------------------------------------------------------------------------------
// The position sweep + per-feature parity assertion
// ----------------------------------------------------------------------------------------------------

// Every byte position of every line of every file: (path-id, package, line, character). Character is a
// UTF-8 byte offset within the line (what the IDE features consume); the curated sources are ASCII.
fn positions(workspace: &Workspace) -> Vec<(FileId, bool, TextPosition)> {
    let mut out = Vec::new();
    for (id, state) in &workspace.files {
        for (line_index, line) in state.source.split('\n').enumerate() {
            for character_index in 0..=line.len() {
                out.push((
                    *id,
                    state.package,
                    TextPosition {
                        line_index,
                        character_index,
                    },
                ));
            }
        }
    }
    out
}

// A location flattened to its comparable fields: path plus start/end line and character.
type LocationKey = (PathBuf, usize, usize, usize, usize);
// An edit flattened to its comparable fields: start/end line and character plus replacement text.
type EditKey = (usize, usize, usize, usize, String);

// LSP location order is not semantic, so location-bearing results are compared as a sorted set.
fn sorted_locations(locations: Option<Vec<Location>>) -> Option<Vec<LocationKey>> {
    locations.map(|mut locations| {
        let mut keyed = locations
            .drain(..)
            .map(|location| {
                (
                    location.path,
                    location.range.start.line_index,
                    location.range.start.character_index,
                    location.range.end.line_index,
                    location.range.end.character_index,
                )
            })
            .collect::<Vec<_>>();
        keyed.sort();
        keyed
    })
}

// Rename edits already group by path (a `BTreeMap`); normalize each path's edit list to a sorted set so
// the comparison ignores within-file occurrence order.
fn sorted_rename(rename: Option<RenameResult>) -> Option<BTreeMap<PathBuf, Vec<EditKey>>> {
    rename.map(|rename| {
        rename
            .edits
            .into_iter()
            .map(|(path, edits)| {
                let mut keyed = edits
                    .into_iter()
                    .map(|edit| {
                        (
                            edit.range.start.line_index,
                            edit.range.start.character_index,
                            edit.range.end.line_index,
                            edit.range.end.character_index,
                            edit.replacement_text,
                        )
                    })
                    .collect::<Vec<_>>();
                keyed.sort();
                (path, keyed)
            })
            .collect()
    })
}

// Cold parity: a fresh engine built from scratch for this exact state.
fn assert_parity(label: &str, workspace: &Workspace) {
    let mut oracle = build_oracle(workspace);
    let (engine, paths) = build_engine(workspace);
    let engine_ide = EngineIde::new(&engine, &paths);
    sweep_parity(label, workspace, &mut oracle, &engine_ide);
}

// Compare every feature at every position of every file between the oracle and one `EngineIde` (cold or
// incrementally updated). Factored so the incremental driver re-sweeps a long-lived engine after each edit.
fn sweep_parity(label: &str, workspace: &Workspace, oracle: &mut Analysis, engine_ide: &EngineIde) {
    // Inlay hints are whole-file (no position); compare once per file.
    for (id, state) in &workspace.files {
        let path = absolute_path(*id, state.package);
        assert_eq!(
            ide::inlay_hints(oracle, &path, None),
            engine_ide.inlay_hints(&path, None),
            "inlay_hints divergence at {label}, file {path:?}",
        );
    }

    for (id, package, position) in positions(workspace) {
        let path = absolute_path(id, package);
        let where_ = format!("{label}, {path:?} {position:?}");

        assert_eq!(
            ide::hover(oracle, &path, position),
            engine_ide.hover(&path, position),
            "hover divergence at {where_}",
        );
        assert_eq!(
            ide::signature_help(oracle, &path, position),
            engine_ide.signature_help(&path, position),
            "signature_help divergence at {where_}",
        );
        assert_eq!(
            ide::completion(oracle, &path, position),
            engine_ide.completion(&path, position),
            "completion divergence at {where_}",
        );
        assert_eq!(
            sorted_locations(ide::definition(oracle, &path, position)),
            sorted_locations(engine_ide.definition(&path, position)),
            "definition divergence at {where_}",
        );
        for include_declaration in [false, true] {
            assert_eq!(
                sorted_locations(ide::references(
                    oracle,
                    &path,
                    position,
                    include_declaration
                )),
                sorted_locations(engine_ide.references(&path, position, include_declaration)),
                "references(include_declaration={include_declaration}) divergence at {where_}",
            );
        }
        assert_eq!(
            sorted_rename(ide::rename(oracle, &path, position, "renamed_symbol")),
            sorted_rename(engine_ide.rename(&path, position, "renamed_symbol")),
            "rename divergence at {where_}",
        );
    }
}

fn typing_config() -> Config {
    Config {
        check: CheckConfig {
            typing: true,
            strict: false,
            unused: false,
        },
        lint: LintConfig::default(),
    }
}

fn workspace(config: Config, files: &[(FileId, bool, &str)]) -> Workspace {
    Workspace {
        files: files
            .iter()
            .map(|(id, package, source)| {
                (
                    *id,
                    FileState {
                        source: (*source).to_owned(),
                        package: *package,
                    },
                )
            })
            .collect(),
        config,
    }
}

// ----------------------------------------------------------------------------------------------------
// Curated scenarios (single-file and cross-file), each swept at every position
// ----------------------------------------------------------------------------------------------------

#[test]
fn single_file_locals_and_types() {
    assert_parity(
        "single-file-locals",
        &workspace(
            typing_config(),
            &[(
                0,
                true,
                "double <- function(count) count + count\nresult <- double(2L)\nlabel <- \"hi\"",
            )],
        ),
    );
}

#[test]
fn cross_file_function_use() {
    // A typed function defined in one package file, called in another: goto/hover/references on
    // `double_count` must land in the definer from the referrer, and vice versa.
    assert_parity(
        "cross-file-function",
        &workspace(
            typing_config(),
            &[
                (
                    0,
                    true,
                    "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count",
                ),
                (
                    1,
                    true,
                    "result <- double_count(2L)\nother <- double_count(3L)",
                ),
            ],
        ),
    );
}

#[test]
fn cross_file_value_and_global_redefinition() {
    // `shared` is defined in two package files (last path wins); references in the user file must
    // resolve to the winning definer on both engines, and rename must rewrite every occurrence.
    assert_parity(
        "cross-file-redefinition",
        &workspace(
            typing_config(),
            &[
                (0, true, "shared <- 1L"),
                (1, true, "shared <- 2L"),
                (2, true, "value <- shared\nagain <- shared"),
            ],
        ),
    );
}

#[test]
fn completion_contexts_and_globals() {
    // Local bindings, package globals, and a `$`/`@`/`::` context cursor — completion parity at every
    // position, including the partial-prefix cursors a client sends mid-identifier.
    assert_parity(
        "completion",
        &workspace(
            typing_config(),
            &[
                (0, true, "alpha_one <- 1L\nalpha_two <- function() NULL"),
                (
                    1,
                    true,
                    "use <- function(record) {\n  local_value <- 1L\n  record$field\n  al\n}",
                ),
            ],
        ),
    );
}

#[test]
fn s4_classes_cross_file() {
    // S4 class/generic names are string literals resolved structurally over every file; goto/references/
    // rename on `"Animal"` and `"speak"` must span the declaring and using files identically.
    assert_parity(
        "s4",
        &workspace(
            typing_config(),
            &[
                (
                    0,
                    true,
                    "setClass(\"Animal\", representation(name = \"character\"))\nsetGeneric(\"speak\", function(x) standardGeneric(\"speak\"))",
                ),
                (
                    1,
                    true,
                    "setMethod(\"speak\", \"Animal\", function(x) \"...\")\npet <- new(\"Animal\", name = \"Rex\")",
                ),
            ],
        ),
    );
}

#[test]
fn script_referencing_package_global() {
    // A script (not a package file) that references a package global: the global resolves, but the
    // script's own bindings stay script-local. Parity must hold across the package/script boundary.
    assert_parity(
        "script-and-package",
        &workspace(
            typing_config(),
            &[
                (
                    0,
                    true,
                    "#: fn(x: integer) -> integer\nhelper <- function(x) x + x",
                ),
                (
                    1,
                    false,
                    "local_result <- helper(2L)\nfollow <- local_result",
                ),
            ],
        ),
    );
}

// ----------------------------------------------------------------------------------------------------
// Incremental parity: one long-lived engine driven through an edit stream, re-swept after every edit
// ----------------------------------------------------------------------------------------------------

// The cold scenarios prove the orchestration; this proves the engine still serves correct IDE output off
// *incrementally invalidated* state. The engine memoizes the facts (parse/lower/naming/typecheck/index);
// the IDE view only reads them, so a divergence here is an invalidation or priming/PathTable-lockstep bug
// (a cache primed from a stale fetch, a path not retracted on delete, a kind not flipped on reclassify).

fn sync_engine(
    engine: &mut Engine<RoughlyQueries>,
    paths: &mut PathTable,
    previous: &Workspace,
    next: &Workspace,
) {
    for id in previous.files.keys() {
        if !next.files.contains_key(id) {
            engine.remove_input(&Key::SourceText(*id));
            engine.remove_input(&Key::DocumentKind(*id));
            engine.remove_input(&Key::FileName(*id));
            paths.remove(*id);
        }
    }
    for (id, state) in &next.files {
        engine.set_input(Key::SourceText(*id), parse_source_input(&state.source));
        engine.set_input(Key::FileName(*id), absolute_path(*id, state.package));
        engine.set_input(
            Key::DocumentKind(*id),
            if state.package {
                analysis::naming::DocumentKind::Package
            } else {
                analysis::naming::DocumentKind::Script
            },
        );
        // `PathTable::insert` retracts the old path when a reclassify changes it, keeping the bijection in
        // lockstep with the `ProjectFiles` input exactly as the production host must.
        paths.insert(*id, absolute_path(*id, state.package));
    }
    engine.set_input(Key::ProjectFiles, project_files_order(next));
    engine.set_input(Key::Config, next.config.clone());
}

struct Driver {
    engine: Engine<RoughlyQueries>,
    paths: PathTable,
    workspace: Workspace,
}

impl Driver {
    fn new(config: Config) -> Driver {
        Driver {
            engine: Engine::new(RoughlyQueries::new()),
            paths: PathTable::new(PathBuf::from(BASE)),
            workspace: Workspace {
                files: BTreeMap::new(),
                config,
            },
        }
    }

    fn step(&mut self, label: &str, mutate: impl FnOnce(&mut Workspace)) {
        let previous = self.workspace.clone();
        mutate(&mut self.workspace);
        sync_engine(
            &mut self.engine,
            &mut self.paths,
            &previous,
            &self.workspace,
        );
        let mut oracle = build_oracle(&self.workspace);
        let engine_ide = EngineIde::new(&self.engine, &self.paths);
        sweep_parity(label, &self.workspace, &mut oracle, &engine_ide);
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
}

#[test]
fn incremental_cross_file_interface_edits() {
    // A referrer whose goto/hover/references resolve into a definer across a stream of interface edits:
    // body edit (scheme unchanged), signature change (referrer's call becomes ill-typed), and a rename of
    // the global. After each, the engine's IDE output must still match a fresh oracle at every position.
    let mut driver = Driver::new(typing_config());
    driver.set_package(
        "define",
        0,
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count + count",
    );
    driver.set_package(
        "use",
        1,
        "result <- double_count(2L)\nalias <- double_count",
    );
    // Body-only edit: the exported scheme is unchanged, but goto/references on `double_count` must still
    // land correctly (the definer's binding range may have moved).
    driver.set_package(
        "edit-body",
        0,
        "#: fn(count: integer) -> integer\ndouble_count <- function(count) count * 2L + count",
    );
    // Rename the global in the definer: the referrer's name now dangles; hover/goto/references must follow.
    driver.set_package(
        "rename-global",
        0,
        "#: fn(count: integer) -> integer\ndoubler <- function(count) count + count",
    );
    // Update the referrer to the new name.
    driver.set_package("fix-referrer", 1, "result <- doubler(2L)\nalias <- doubler");
}

#[test]
fn incremental_add_delete_and_reclassify() {
    // File lifecycle under IDE queries: add a definer and a user, delete the definer (references collapse to
    // the user file), re-add it (cross-file resolution returns), then reclassify the user to a script
    // (still references the package global). Parity is swept after every step, exercising PathTable lockstep.
    let mut driver = Driver::new(typing_config());
    driver.set_package("add-def", 0, "shared <- 1L");
    driver.set_package("add-use", 1, "value <- shared\nagain <- shared");
    // Delete the definer: `shared` is no longer a package global, so the uses dangle (local-unresolved) on
    // both engines, and references/goto must reflect that identically.
    driver.delete("delete-def", 0);
    // Re-add the same slot (tombstone re-add): cross-file resolution must return.
    driver.set_package("readd-def", 0, "shared <- 2L");
    // Reclassify the user as a script: it still references the package global `shared`.
    driver.set_script("reclassify-use", 1, "value <- shared\nagain <- shared");
}

// ----------------------------------------------------------------------------------------------------
// Randomized IDE differential stream (mirrors the diagnostic harness's adversarial stream)
// ----------------------------------------------------------------------------------------------------

// The curated incremental scenarios cover the obvious invalidation shapes; this drives a fixed-seed random
// edit stream (add/edit/delete/reclassify over a small cross-file workspace) and sweeps full per-position
// IDE parity after every step, catching the long-tail invalidation bugs the curated cases miss — the IDE
// analog of the diagnostic harness's `randomized_adversarial_edit_stream`. Small workspace + modest step
// count because each step re-sweeps every position of every file across all seven features.

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

const NAMES: [&str; 3] = ["alpha", "beta", "gamma"];

// Sources with real IDE targets: typed function defs, cross-file calls and value references, and a local
// binding inside a function body — so hover/goto/refs/rename/signature/inlay/completion all have something
// to resolve, and references draw freely from the pool so cross-file resolution forms and dissolves.
fn generate_ide_source(rng: &mut SplitMix64) -> String {
    let name = NAMES[rng.below(NAMES.len() as u64) as usize];
    let target = NAMES[rng.below(NAMES.len() as u64) as usize];
    match rng.below(5) {
        0 => format!("#: fn(x: integer) -> integer\n{name} <- function(x) x + x"),
        1 => format!("{name} <- {target}(1L)"),
        2 => format!("result <- {target}\nalias <- {target}"),
        3 => format!("{name} <- function() {{\n  local_value <- {target}(1L)\n  local_value\n}}"),
        _ => format!("{name} <- 1L"),
    }
}

const MAX_SLOTS: FileId = 3;

#[test]
fn randomized_ide_differential_stream() {
    for seed in [0x1234_5678u64, 0xDEAD_BEEF, 42] {
        let mut rng = SplitMix64::new(seed);
        let mut driver = Driver::new(typing_config());

        for step in 0..25 {
            let label = format!("seed={seed:#x} step={step}");
            let slot = rng.below(MAX_SLOTS as u64) as FileId;
            let present = driver.workspace.files.contains_key(&slot);

            match rng.below(6) {
                0 if present => driver.delete(&label, slot),
                1 if present => {
                    // Reclassify, keeping the source.
                    let source = driver.workspace.files[&slot].source.clone();
                    let package = !driver.workspace.files[&slot].package;
                    driver.step(&label, |workspace| {
                        workspace.files.insert(slot, FileState { source, package });
                    });
                }
                _ => {
                    let source = generate_ide_source(&mut rng);
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
}
