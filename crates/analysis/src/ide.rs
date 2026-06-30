//! The IDE feature surface: result types, the [`IdeDatabase`] fact-provider trait the interactive
//! features read through, and the `Analysis`-backed entry points the LSP server and fixtures call.
//!
//! The features themselves live in [`generic`], written once over `&dyn IdeDatabase` so the identical
//! orchestration serves both `Analysis` (the retained-state oracle, implemented here) and the
//! engine-backed view (`engine::ide_view`). The public `hover`/`completion`/… functions below are thin
//! wrappers: they drive `Analysis`'s phases up to the freshness each feature needs, then delegate to the
//! matching `generic::*` function. Keeping these signatures (`&mut Analysis`) stable means the server and
//! the fixture harness are unaffected by the generic split.
pub mod generic;

pub use generic::{MatchScore, search_match};

use {
    crate::{
        analysis::{Analysis, lower, resolve_package, typecheck},
        document::{Document, DocumentId},
        hir::{ExpressionId, Module},
        interner::Interner,
        naming::{NamesGlobal, NamesLocal},
        text::{TextPosition, TextRange},
        types::CoreType,
    },
    std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    },
};

/// The facts the interactive IDE features read. `Analysis` serves them from its retained incremental
/// state (below); the engine serves the same facts from its memoized query graph. Every method is a
/// pure read of an already-computed fact — the implementor is responsible for having brought the
/// relevant phases up to date before a feature is invoked (the `Analysis` wrappers run the phases; the
/// engine view primes its caches).
pub trait IdeDatabase {
    fn interner(&self) -> &Interner;
    fn base_path(&self) -> &Path;
    fn document_id_for_path(&self, path: &Path) -> Option<DocumentId>;
    fn path_for_document_id(&self, document_id: DocumentId) -> Option<&Path>;
    fn document_by_id(&self, document_id: DocumentId) -> Option<&Document>;
    fn module(&self, document_id: DocumentId) -> Option<&Module>;
    fn document_naming(&self, document_id: DocumentId) -> Option<&NamesLocal>;
    fn package_naming(&self) -> Option<&NamesGlobal>;
    fn checked_expression_type(
        &self,
        document_id: DocumentId,
        expression_id: ExpressionId,
    ) -> Option<&CoreType>;
    fn all_document_ids(&self) -> Vec<DocumentId>;
}

// `Analysis`'s inherent accessors already expose every fact the trait names, so the impl forwards to
// them (inherent methods take precedence in method resolution, so each `self.x()` calls the inherent
// accessor, not the trait method — no recursion).
impl IdeDatabase for Analysis {
    fn interner(&self) -> &Interner {
        self.interner()
    }

    fn base_path(&self) -> &Path {
        self.base_path()
    }

    fn document_id_for_path(&self, path: &Path) -> Option<DocumentId> {
        self.document_id_for_path(path)
    }

    fn path_for_document_id(&self, document_id: DocumentId) -> Option<&Path> {
        self.path_for_document_id(document_id)
    }

    fn document_by_id(&self, document_id: DocumentId) -> Option<&Document> {
        self.document_by_id(document_id)
    }

    fn module(&self, document_id: DocumentId) -> Option<&Module> {
        self.module(document_id)
    }

    fn document_naming(&self, document_id: DocumentId) -> Option<&NamesLocal> {
        self.document_naming(document_id)
    }

    fn package_naming(&self) -> Option<&NamesGlobal> {
        self.package_naming()
    }

    fn checked_expression_type(
        &self,
        document_id: DocumentId,
        expression_id: ExpressionId,
    ) -> Option<&CoreType> {
        self.checked_expression_type(document_id, expression_id)
    }

    fn all_document_ids(&self) -> Vec<DocumentId> {
        self.all_document_ids()
    }
}

//
// Hover
//

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub range: TextRange,
    // Primary, human-readable, unnamed markdown blocks shown by default: the inferred type and, for
    // a variable use, where it is defined and whether it is local or package-global.
    pub contents: Vec<String>,
    // Phase-by-phase internal facts shown only under a named `Debug` heading when debug is enabled.
    pub debug: Vec<DebugSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugSection {
    pub title: String,
    pub body: String,
}

pub fn hover(analysis: &mut Analysis, path: &Path, position: TextPosition) -> Option<HoverInfo> {
    lower(analysis);
    resolve_package(analysis);
    // Typed hover needs checked expression types, which only `typecheck` retains. The typing phase
    // runs on demand for IDE features regardless of whether type-error diagnostics are surfaced; it
    // is incremental, so this is cheap when the package is already fresh.
    typecheck(analysis);
    generic::hover(&*analysis, path, position)
}

pub fn render_hover_markdown(hover_info: &HoverInfo, include_debug: bool) -> String {
    let mut rendered = hover_info.contents.join("\n\n");

    if include_debug && !hover_info.debug.is_empty() {
        let debug_body = hover_info
            .debug
            .iter()
            .map(|section| format!("**{}**\n\n{}", section.title, section.body))
            .collect::<Vec<_>>()
            .join("\n\n");
        rendered.push_str("\n\n---\n\n### Debug\n\n");
        rendered.push_str(&debug_body);
    }

    rendered
}

//
// Inlay hints
//

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub position: TextPosition,
    pub label: String,
}

pub fn inlay_hints(
    analysis: &mut Analysis,
    path: &Path,
    viewport: Option<TextRange>,
) -> Vec<InlayHint> {
    lower(analysis);
    resolve_package(analysis);
    typecheck(analysis);
    generic::inlay_hints(&*analysis, path, viewport)
}

//
// Signature help
//

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    pub label: String,
    pub parameters: Vec<String>,
    pub active_parameter: Option<usize>,
}

pub fn signature_help(
    analysis: &mut Analysis,
    path: &Path,
    position: TextPosition,
) -> Option<SignatureHelp> {
    lower(analysis);
    resolve_package(analysis);
    typecheck(analysis);
    generic::signature_help(&*analysis, path, position)
}

//
// Definition
//

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: TextRange,
}

pub fn definition(
    analysis: &mut Analysis,
    path: &Path,
    position: TextPosition,
) -> Option<Vec<Location>> {
    lower(analysis);
    resolve_package(analysis);
    generic::definition(&*analysis, path, position)
}

//
// References
//

pub fn references(
    analysis: &mut Analysis,
    path: &Path,
    position: TextPosition,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    lower(analysis);
    resolve_package(analysis);
    generic::references(&*analysis, path, position, include_declaration)
}

//
// Rename
//

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameResult {
    pub edits: BTreeMap<PathBuf, Vec<RenameEdit>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameEdit {
    pub range: TextRange,
    pub replacement_text: String,
}

pub fn rename(
    analysis: &mut Analysis,
    path: &Path,
    position: TextPosition,
    new_name: &str,
) -> Option<RenameResult> {
    lower(analysis);
    resolve_package(analysis);
    generic::rename(&*analysis, path, position, new_name)
}

//
// Completion
//

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionItemKind,
    pub source: CompletionItemSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionItemKind {
    Keyword,
    Variable,
    Function,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionItemSource {
    Keyword,
    Local,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResult {
    pub items: Vec<CompletionItem>,
    // True when the candidate set was capped at `COMPLETION_LIMIT`; the server forwards this as the
    // LSP `isIncomplete` flag so the client keeps re-querying as the prefix narrows instead of
    // filtering a stale, truncated list locally.
    pub is_incomplete: bool,
}

// Matches the workspace-symbol cap. The full global namespace can exceed 20k entries; returning it
// all produces a huge payload and lets the client cache a complete list and stop re-querying.
pub const COMPLETION_LIMIT: usize = 128;

pub fn completion(
    analysis: &mut Analysis,
    path: &Path,
    position: TextPosition,
) -> Option<CompletionResult> {
    lower(analysis);
    resolve_package(analysis);
    generic::completion(&*analysis, path, position)
}

#[cfg(test)]
mod completion_limit_tests {
    use {
        super::{COMPLETION_LIMIT, completion},
        crate::{
            analysis::{Analysis, CheckConfig, LintConfig},
            text::TextPosition,
        },
        std::path::{Path, PathBuf},
    };

    fn analysis_with_globals(count: usize) -> Analysis {
        let mut source = String::new();
        for index in 0..count {
            source.push_str(&format!("g{index:04} <- function() NULL\n"));
        }

        let mut analysis = Analysis::new(PathBuf::new(), LintConfig::default(), CheckConfig::default());
        analysis
            .add_document_from_source(PathBuf::from("R/globals.R"), &source)
            .expect("globals parse");
        // A bare prefix to complete against the global namespace.
        analysis
            .add_document_from_source(PathBuf::from("R/main.R"), "g\n")
            .expect("main parse");
        analysis
    }

    fn complete_prefix_g(analysis: &mut Analysis) -> super::CompletionResult {
        completion(
            analysis,
            Path::new("R/main.R"),
            TextPosition { line_index: 0, character_index: 1 },
        )
        .expect("completions present")
    }

    #[test]
    fn caps_at_limit_and_marks_incomplete() {
        let mut analysis = analysis_with_globals(COMPLETION_LIMIT + 10);
        let result = complete_prefix_g(&mut analysis);

        assert_eq!(result.items.len(), COMPLETION_LIMIT);
        assert!(result.is_incomplete);
    }

    #[test]
    fn returns_all_when_under_limit() {
        let count = 5;
        let mut analysis = analysis_with_globals(count);
        let result = complete_prefix_g(&mut analysis);

        assert_eq!(result.items.len(), count);
        assert!(!result.is_incomplete);
    }
}

#[cfg(test)]
mod inlay_viewport_tests {
    use {
        super::inlay_hints,
        crate::{
            analysis::{Analysis, CheckConfig, LintConfig},
            text::{TextPosition, TextRange},
        },
        std::path::{Path, PathBuf},
    };

    fn analysis_with_three_bindings() -> Analysis {
        let mut analysis = Analysis::new(PathBuf::new(), LintConfig::default(), CheckConfig::default());
        analysis
            .add_document_from_source(
                PathBuf::from("R/main.R"),
                "count <- 1L\nlabel <- \"hello\"\nratio <- 2L\n",
            )
            .expect("source parses");
        analysis
    }

    #[test]
    fn full_document_returns_all_hints() {
        let mut analysis = analysis_with_three_bindings();
        let hints = inlay_hints(&mut analysis, Path::new("R/main.R"), None);

        let lines: Vec<usize> = hints.iter().map(|hint| hint.position.line_index).collect();
        assert_eq!(lines, vec![0, 1, 2]);
    }

    #[test]
    fn viewport_excludes_hints_outside_range() {
        let mut analysis = analysis_with_three_bindings();
        // Cover only the middle line; the surrounding bindings must drop out.
        let viewport = TextRange {
            start: TextPosition { line_index: 1, character_index: 0 },
            end: TextPosition { line_index: 1, character_index: 99 },
        };
        let hints = inlay_hints(&mut analysis, Path::new("R/main.R"), Some(viewport));

        let lines: Vec<usize> = hints.iter().map(|hint| hint.position.line_index).collect();
        assert_eq!(lines, vec![1]);
    }
}
