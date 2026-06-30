//! Symbols differential (cutover Phase 2/3, done-bar (b) for the 8th IDE feature). Document and workspace
//! symbols are purely structural — `roughly::symbols` runs `index::index` over a document's tree+rope and
//! never touches the analysis pipeline — so the only thing the cutover must preserve is that the engine
//! serves the *same* document trees and the *same* package-document set as `analysis`. This asserts exactly
//! that: symbol output built from engine-served parses equals output built from the `analysis` oracle, for
//! document symbols (per file) and workspace symbols (ranked, over several queries).

use {
    analysis::{Analysis, CheckConfig, LintConfig, document::DocumentId, naming::DocumentKind},
    engine::{
        Engine,
        ide_view::PathTable,
        queries::{Config, FileId, Key, ParsedDocument, RoughlyQueries},
    },
    roughly::{
        index::{self, Item},
        lsp_types::{Position, Range},
        symbols,
    },
    std::{collections::HashMap, path::PathBuf},
};

const BASE: &str = "/ws";

fn package_path(id: FileId) -> PathBuf {
    PathBuf::from(format!("{BASE}/R/f{id:04}.R"))
}

// A shared structural source exercising every symbol kind index() emits: top-level functions and values,
// nested locals, an S4 class/generic/method, and an R6 class — across several package files.
fn workspace_files() -> Vec<(FileId, &'static str)> {
    vec![
        (
            0,
            "instrument <- function(name, value) {\n  helper <- function(x) x + 1\n  helper(value)\n}\nconstant <- 42L",
        ),
        (
            1,
            "setClass(\"Animal\", representation(name = \"character\"))\nsetGeneric(\"speak\", function(x) standardGeneric(\"speak\"))\nsetMethod(\"speak\", \"Animal\", function(x) \"...\")",
        ),
        (
            2,
            "Counter <- R6::R6Class(\"Counter\", public = list(count = 0, increment = function() self$count <- self$count + 1))",
        ),
    ]
}

fn items_for(tree_root: tree_sitter::Node, rope: &ropey::Rope) -> Vec<Item> {
    index::index(tree_root, rope, false, false)
}

fn analysis_package_items(analysis: &Analysis) -> HashMap<PathBuf, Vec<Item>> {
    analysis
        .package_document_ids()
        .into_iter()
        .map(|document_id| {
            let path = analysis
                .path_for_document_id(document_id)
                .expect("oracle path")
                .to_path_buf();
            let document = analysis
                .document_by_id(document_id)
                .expect("oracle document");
            (
                path,
                items_for(document.tree().root_node(), document.rope()),
            )
        })
        .collect()
}

fn engine_package_items(
    engine: &Engine<RoughlyQueries>,
    paths: &PathTable,
    package_ids: &[FileId],
) -> HashMap<PathBuf, Vec<Item>> {
    package_ids
        .iter()
        .map(|file| {
            let parsed = engine.fetch::<ParsedDocument>(Key::Parse(*file));
            let path = paths
                .path(DocumentId(*file))
                .expect("engine path")
                .to_path_buf();
            (
                path,
                items_for(parsed.0.tree().root_node(), parsed.0.rope()),
            )
        })
        .collect()
}

// A trivial TextRange -> LSP Range converter. Parity does not depend on the encoding, only on both sides
// using the same converter over identical item ranges, so byte columns are passed straight through.
fn to_lsp_range(range: analysis::TextRange) -> Range {
    Range::new(
        Position::new(
            range.start.line_index as u32,
            range.start.character_index as u32,
        ),
        Position::new(
            range.end.line_index as u32,
            range.end.character_index as u32,
        ),
    )
}

#[test]
fn document_and_workspace_symbols_match_the_analysis_oracle() {
    let files = workspace_files();
    let package_ids = files.iter().map(|(id, _)| *id).collect::<Vec<_>>();

    // Oracle: a fresh Analysis with the package documents loaded.
    let mut oracle = Analysis::new(
        PathBuf::from(BASE),
        LintConfig::default(),
        CheckConfig::default(),
    );
    for (id, source) in &files {
        oracle
            .add_document_from_source(package_path(*id), source)
            .expect("oracle source should parse");
    }

    // Engine: the same files as inputs, with the host PathTable in lockstep.
    let mut engine = Engine::new(RoughlyQueries::new());
    let mut paths = PathTable::new(PathBuf::from(BASE));
    for (id, source) in &files {
        engine.set_input(Key::SourceText(*id), (*source).to_owned());
        engine.set_input(Key::DocumentKind(*id), DocumentKind::Package);
        paths.insert(*id, package_path(*id));
    }
    engine.set_input(Key::ProjectFiles, package_ids.clone());
    engine.set_input(Key::Config, Config::default());

    let oracle_map = analysis_package_items(&oracle);
    let engine_map = engine_package_items(&engine, &paths, &package_ids);

    // Document symbols: identical per file (same tree -> same index() -> same symbols).
    for (id, _) in &files {
        let path = package_path(*id);
        assert_eq!(
            symbols::document(&oracle_map[&path], &to_lsp_range),
            symbols::document(&engine_map[&path], &to_lsp_range),
            "document symbols diverge for {path:?}",
        );
    }

    // Workspace symbols: identical ranked output over a range of queries (empty = everything, exact,
    // subsequence, and a non-match).
    let to_lsp_in = |_path: &std::path::Path, range: analysis::TextRange| to_lsp_range(range);
    for query in ["", "instrument", "spk", "Counter", "increment", "zzz"] {
        assert_eq!(
            symbols::workspace(query, &oracle_map, &to_lsp_in),
            symbols::workspace(query, &engine_map, &to_lsp_in),
            "workspace symbols diverge for query {query:?}",
        );
    }
}
