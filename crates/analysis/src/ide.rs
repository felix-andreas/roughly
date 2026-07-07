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
        interner::{Interner, Symbol},
        naming::{DocumentKind, NamesGlobal, NamesLocal},
        text::{TextPosition, TextRange},
        types::{Constraint, CoreType, InferenceVariableId, TypeScheme},
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
    // The constraint of a still-unbound inference variable in a stored expression type, so
    // display-time generalization renders `<T: numeric>` rather than a bare `<T>`.
    fn variable_constraint(
        &self,
        document_id: DocumentId,
        variable: InferenceVariableId,
    ) -> Constraint;
    fn all_document_ids(&self) -> Vec<DocumentId>;
    // Whether the document participates in the package (top-level bindings are package globals) or is
    // a standalone script (top-level bindings are document-local slots). Completion offers a script's
    // top-level bindings as locals; a package file's arrive through the global namespace instead.
    fn document_kind(&self, document_id: DocumentId) -> Option<DocumentKind>;
    // The namespace a symbol's stub was declared in (`base`, `stats`, …, or a project stub file's
    // namespace), for showing the name's origin package on hover. `None` when the symbol is not a stub.
    fn stub_namespace(&self, symbol: Symbol) -> Option<Symbol>;
    // Every standard-library stub name with its scheme, for the stdlib completion source.
    fn stub_schemes(&self) -> Vec<(Symbol, &TypeScheme)>;
    // The full declared overload set of a stub name, in declaration order; empty for non-stubs.
    // Signature help lists every candidate of a multi-scheme name.
    fn stub_overload_schemes(&self, symbol: Symbol) -> &[TypeScheme];
    // The declared-set index of the overload a call committed, keyed by the callee expression;
    // `None` when the callee did not resolve to a stub overload set or the call never matched.
    fn selected_overload(
        &self,
        document_id: DocumentId,
        expression_id: ExpressionId,
    ) -> Option<usize>;
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

    fn variable_constraint(
        &self,
        document_id: DocumentId,
        variable: InferenceVariableId,
    ) -> Constraint {
        self.variable_constraint(document_id, variable)
    }

    fn all_document_ids(&self) -> Vec<DocumentId> {
        self.all_document_ids()
    }

    fn document_kind(&self, document_id: DocumentId) -> Option<DocumentKind> {
        self.document_kind(document_id)
    }

    fn stub_namespace(&self, symbol: Symbol) -> Option<Symbol> {
        self.stub_namespace(symbol)
    }

    fn stub_schemes(&self) -> Vec<(Symbol, &TypeScheme)> {
        self.stub_schemes()
    }

    fn stub_overload_schemes(&self, symbol: Symbol) -> &[TypeScheme] {
        self.stub_overload_schemes(symbol)
    }

    fn selected_overload(
        &self,
        document_id: DocumentId,
        expression_id: ExpressionId,
    ) -> Option<usize> {
        self.selected_overload(document_id, expression_id)
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
    // One entry per signature. Most calls have exactly one; a call whose callee resolved to a stub
    // overload set lists every declared candidate so the editor can page through them.
    pub signatures: Vec<SignatureData>,
    // Index into `signatures` of the committed overload (0 for single-signature calls).
    pub active_signature: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureData {
    pub label: String,
    // Byte spans into `label`, one per parameter in display order (positional, named, then `...`
    // when variadic). Spans rather than copied strings keep the label the single source of truth,
    // so the LSP layer's parameter-label offsets can never drift from the rendered signature.
    pub parameters: Vec<std::ops::Range<usize>>,
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
    pub edits: BTreeMap<PathBuf, Vec<TextEdit>>,
}

/// One replacement in a document: rename occurrences and code-action edits share this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
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
// Code actions
//

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeAction {
    pub title: String,
    pub kind: CodeActionKind,
    // All current actions are static single-file edits (no resolve round-trip); the map shape
    // matches `RenameResult` so the LSP layer converts both identically.
    pub edits: BTreeMap<PathBuf, Vec<TextEdit>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeActionKind {
    RemoveUnusedAssignment,
    PrefixDot,
    InsertInferredAnnotation,
    AddMissingAnnotations,
    IfUnknownToTrust,
}

impl CodeActionKind {
    // The hierarchical LSP kind string. Clients filter by dot-separated prefix, so each fix is a
    // sub-kind of the standard `quickfix` / `source` base kinds.
    pub fn as_lsp_string(self) -> &'static str {
        match self {
            Self::RemoveUnusedAssignment => "quickfix.remove-unused-assignment",
            Self::PrefixDot => "quickfix.prefix-dot",
            Self::InsertInferredAnnotation => "quickfix.insert-inferred-type-annotation",
            Self::AddMissingAnnotations => "source.addMissingAnnotations",
            Self::IfUnknownToTrust => "quickfix.if-unknown-to-trust",
        }
    }
}

pub fn code_actions(analysis: &mut Analysis, path: &Path, range: TextRange) -> Vec<CodeAction> {
    lower(analysis);
    resolve_package(analysis);
    // The annotation actions read checked types (the inserted text is the inlay-hint rendering).
    typecheck(analysis);
    generic::code_actions(&*analysis, path, range)
}

//
// Type definition
//

pub fn type_definition(
    analysis: &mut Analysis,
    path: &Path,
    position: TextPosition,
) -> Option<Vec<Location>> {
    lower(analysis);
    resolve_package(analysis);
    typecheck(analysis);
    generic::type_definition(&*analysis, path, position)
}

//
// Completion
//

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionItemKind,
    pub source: CompletionItemSource,
    // Extra type information shown next to the item: a stub's or typed field's rendered type, or a
    // type name's declaring directive. `None` when the item carries no type fact.
    pub detail: Option<String>,
    // Prose shown in the item's documentation panel — a stdlib item's origin package. `None` when
    // the item has nothing beyond its label and detail.
    pub documentation: Option<String>,
    // Whether a `Function` item's call takes any arguments (parameters, named parameters, or a
    // rest parameter), when its signature is known. Drives the call-snippet shape a
    // snippet-capable client inserts: `name($0)` with arguments, `name()$0` without, and the
    // with-arguments default when unknown. `None` for non-functions.
    pub takes_arguments: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionItemKind {
    Keyword,
    Variable,
    Function,
    Field,
    Type,
}

// Where a completion item came from; the variant order is the ranking order among items of equal
// match quality (project names before standard-library names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionItemSource {
    Keyword,
    Local,
    Global,
    Stdlib,
    Field,
    Type,
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
    // Typed field completion (`record$` and `record[["`) reads the subject's checked type.
    typecheck(analysis);
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

        let mut analysis = Analysis::new(
            PathBuf::new(),
            LintConfig::default(),
            CheckConfig::default(),
        );
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
            TextPosition {
                line_index: 0,
                character_index: 1,
            },
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

        // Stdlib stub names matching the prefix also surface; the under-limit guarantee this test
        // pins is that every package global made it through, with the result not truncated.
        let global_items = result
            .items
            .iter()
            .filter(|item| matches!(item.source, super::CompletionItemSource::Global))
            .count();
        assert_eq!(global_items, count);
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
        let mut analysis = Analysis::new(
            PathBuf::new(),
            LintConfig::default(),
            CheckConfig::default(),
        );
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
            start: TextPosition {
                line_index: 1,
                character_index: 0,
            },
            end: TextPosition {
                line_index: 1,
                character_index: 99,
            },
        };
        let hints = inlay_hints(&mut analysis, Path::new("R/main.R"), Some(viewport));

        let lines: Vec<usize> = hints.iter().map(|hint| hint.position.line_index).collect();
        assert_eq!(lines, vec![1]);
    }
}

#[cfg(test)]
mod stub_namespace_hover_tests {
    use {
        super::hover,
        crate::{
            analysis::{Analysis, CheckConfig, LintConfig},
            render_hover_markdown,
            text::TextPosition,
        },
        std::{
            path::PathBuf,
            sync::atomic::{AtomicU64, Ordering},
            time::{SystemTime, UNIX_EPOCH},
        },
    };

    // A base path in the system temp dir with no `stubs/` directory, so project-override discovery finds
    // nothing and the shipped stubs keep their origin namespace. (A `PathBuf::new()` base would resolve
    // the relative `stubs/` against the test's working directory, picking the crate's own shipped stubs
    // up as "overrides" and dropping their namespace tag.)
    fn isolated_base_path() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "roughly-hover-ns-{nanos}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn stdlib_function_hover_shows_origin_package() {
        let mut analysis = Analysis::new(
            isolated_base_path(),
            LintConfig::default(),
            CheckConfig {
                typing: true,
                ..CheckConfig::default()
            },
        );
        analysis
            .add_document_from_source(PathBuf::from("R/main.R"), "lapply\n")
            .expect("document should parse");

        let info = hover(
            &mut analysis,
            &PathBuf::from("R/main.R"),
            TextPosition {
                line_index: 0,
                character_index: 0,
            },
        )
        .expect("hover over a stdlib function should produce a result");
        let rendered = render_hover_markdown(&info, false);
        assert!(
            rendered.contains("From the `base` package."),
            "hovering a base stdlib function should note its origin package, got:\n{rendered}"
        );
    }
}
