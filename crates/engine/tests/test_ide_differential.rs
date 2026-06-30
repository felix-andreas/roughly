//! The IDE differential gate (cutover Phase 2, done-bar (b)): the engine-backed [`EngineIde`] view must
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
        queries::{Config, FileId, Key, RoughlyQueries},
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
        unused: config.unused,
        typing: config.typing,
        strict: config.strict,
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
        engine.set_input(Key::SourceText(*id), state.source.clone());
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

// LSP location order is not semantic, so location-bearing results are compared as a sorted set.
fn sorted_locations(locations: Option<Vec<Location>>) -> Option<Vec<(PathBuf, usize, usize, usize, usize)>> {
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
fn sorted_rename(rename: Option<RenameResult>) -> Option<BTreeMap<PathBuf, Vec<(usize, usize, usize, usize, String)>>> {
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

fn assert_parity(label: &str, workspace: &Workspace) {
    let mut oracle = build_oracle(workspace);
    let (engine, paths) = build_engine(workspace);
    let engine_ide = EngineIde::new(&engine, &paths);

    // Inlay hints are whole-file (no position); compare once per file.
    for (id, state) in &workspace.files {
        let path = absolute_path(*id, state.package);
        assert_eq!(
            ide::inlay_hints(&mut oracle, &path, None),
            engine_ide.inlay_hints(&path, None),
            "inlay_hints divergence at {label}, file {path:?}",
        );
    }

    for (id, package, position) in positions(workspace) {
        let path = absolute_path(id, package);
        let where_ = format!("{label}, {path:?} {position:?}");

        assert_eq!(
            ide::hover(&mut oracle, &path, position),
            engine_ide.hover(&path, position),
            "hover divergence at {where_}",
        );
        assert_eq!(
            ide::signature_help(&mut oracle, &path, position),
            engine_ide.signature_help(&path, position),
            "signature_help divergence at {where_}",
        );
        assert_eq!(
            ide::completion(&mut oracle, &path, position),
            engine_ide.completion(&path, position),
            "completion divergence at {where_}",
        );
        assert_eq!(
            sorted_locations(ide::definition(&mut oracle, &path, position)),
            sorted_locations(engine_ide.definition(&path, position)),
            "definition divergence at {where_}",
        );
        for include_declaration in [false, true] {
            assert_eq!(
                sorted_locations(ide::references(&mut oracle, &path, position, include_declaration)),
                sorted_locations(engine_ide.references(&path, position, include_declaration)),
                "references(include_declaration={include_declaration}) divergence at {where_}",
            );
        }
        assert_eq!(
            sorted_rename(ide::rename(&mut oracle, &path, position, "renamed_symbol")),
            sorted_rename(engine_ide.rename(&path, position, "renamed_symbol")),
            "rename divergence at {where_}",
        );
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
                (1, true, "result <- double_count(2L)\nother <- double_count(3L)"),
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
                (1, false, "local_result <- helper(2L)\nfollow <- local_result"),
            ],
        ),
    );
}
