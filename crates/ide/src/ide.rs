//! IDE features over the semantics crate's salsa queries. Every feature is a
//! pure read of the database at a byte offset; UTF-16 and editor-protocol
//! concerns live in the server, not here.
//!
//! Positions cross one boundary: per-item query results carry item-relative
//! ranges (the position-independent unit salsa cutoffs work on), while the
//! feature API speaks file-absolute offsets. `semantics::item_node` is the
//! edge that anchors an item at its current absolute position.
//!
//! Goto-definition, references, and rename share ONE occurrence engine: the
//! cursor resolves to a symbol target (a variable slot inside an item, or a
//! project-defined global name), and every feature is a projection of that
//! target's occurrence list.

use semantics::diagnostics::TypeRenderer;
use semantics::hir::{Argument, ExprId, ExpressionKind};
use semantics::naming::{BindingId, BindingKind};
use semantics::types::{FunctionType, Ty, TyKind, TypeScheme};
use semantics::{
    Db, Item, ItemKind, ProjectFiles, SourceFile, item_annotation_syntax, item_check, item_hir,
    item_naming, item_node, item_tree, package_definitions,
};
use syntax::{TextRange, TextSize};

/// A hover result: the hovered expression's absolute range and the rendered
/// lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    pub range: TextRange,
    pub lines: Vec<String>,
}

/// A navigation target: a file and an absolute range inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavigationTarget {
    pub file: SourceFile,
    pub range: TextRange,
}

/// One occurrence of a symbol: a range plus whether it is a declaration
/// (an assignment target, a parameter, a for-loop variable, or a top-level
/// definition's name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Occurrence {
    pub file: SourceFile,
    pub range: TextRange,
    pub is_declaration: bool,
}

pub fn hover(db: &dyn Db, file: SourceFile, offset: TextSize) -> Option<Hover> {
    // A cursor inside a `#:` annotation resolves against the type notation,
    // not the R expression it decorates.
    if let Some(cursor) = annotation_type_at(db, file, offset) {
        return annotation_type_hover(db, file, &cursor);
    }
    // Elsewhere inside a `@type`/`@alias` block (the `@`, the keyword, the
    // braces), the whole declaration hovers.
    if let Some(hover) = annotation_definition_hover(db, file, offset) {
        return Some(hover);
    }

    let position = position_in_item(db, file, offset)?;
    let check = item_check(db, position.item)?;
    let hir = item_hir(db, position.item)?;
    // The smallest containing expression with a recorded type: write targets
    // and operators record none, so the hover widens to the enclosing typed
    // expression instead of going silent.
    let (expression, ty) = position
        .expressions_at()
        .into_iter()
        .find_map(|id| check.expression_types.get(&id).map(|ty| (id, *ty)))?;

    let mut renderer = TypeRenderer::default();
    let line = match &hir.expression(expression).kind {
        ExpressionKind::NameRef(name) => {
            format!("{name}: {}", renderer.render(db, ty))
        }
        _ => renderer.render(db, ty),
    };

    // Expression nodes may swallow trailing trivia; the hover highlight must
    // stop at the significant end.
    let range = trim_trailing_trivia(
        file.text(db),
        hir.expression(expression).range + position.item_offset,
    );
    Some(Hover {
        range,
        lines: vec![line],
    })
}

/// Shrinks a range's end past any trailing whitespace/newlines it swallowed.
fn trim_trailing_trivia(text: &str, range: TextRange) -> TextRange {
    let start = usize::from(range.start()).min(text.len());
    let end = usize::from(range.end()).min(text.len());
    let trimmed = text[start..end].trim_end().len();
    TextRange::new(range.start(), TextSize::from((start + trimmed) as u32))
}

/// Goto-definition: a slot goes to its first write; a global goes to the
/// package winner definition's name (or, in a script, the first declaration).
pub fn definition(
    db: &dyn Db,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
) -> Option<NavigationTarget> {
    match target_at(db, files, file, offset)? {
        Target::Slot { item, binding } => {
            let naming = item_naming(db, item)?;
            let info = naming.bindings.get(&binding)?;
            let item_offset = item_node(db, item)?.text_range().start();
            Some(NavigationTarget {
                file,
                range: info.range + item_offset,
            })
        }
        // The LAST declaration wins, matching the project definition table's
        // fold order.
        Target::TypeName(name) => type_name_occurrences(db, files, &name)
            .into_iter()
            .rfind(|occurrence| occurrence.is_declaration)
            .map(|occurrence| NavigationTarget {
                file: occurrence.file,
                range: occurrence.range,
            }),
        ref target @ Target::S4 { .. } => occurrences(db, files, target)
            .into_iter()
            .find(|occurrence| occurrence.is_declaration)
            .map(|occurrence| NavigationTarget {
                file: occurrence.file,
                range: occurrence.range,
            }),
        Target::Global(name) => {
            if let Some(winner) = package_definitions(db, files).get(&name) {
                let node = item_node(db, *winner)?;
                let range = definition_name_range(&node).unwrap_or_else(|| node.text_range());
                return Some(NavigationTarget {
                    file: *winner.file(db),
                    range,
                });
            }
            occurrences(db, files, &Target::Global(name))
                .into_iter()
                .find(|occurrence| occurrence.is_declaration)
                .map(|occurrence| NavigationTarget {
                    file: occurrence.file,
                    range: occurrence.range,
                })
        }
    }
}

/// All occurrences of the symbol under the cursor, in file/source order.
pub fn references(
    db: &dyn Db,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
    include_declaration: bool,
) -> Vec<Occurrence> {
    let Some(target) = target_at(db, files, file, offset) else {
        return Vec::new();
    };
    occurrences(db, files, &target)
        .into_iter()
        .filter(|occurrence| include_declaration || !occurrence.is_declaration)
        .collect()
}

/// Rename: every occurrence becomes an edit site for the new name. `None`
/// when the cursor is not on a renameable symbol (stub and unresolved names
/// have no project declaration to rename).
pub fn rename(
    db: &dyn Db,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
) -> Option<Vec<Occurrence>> {
    let target = target_at(db, files, file, offset)?;
    let occurrences = occurrences(db, files, &target);
    (!occurrences.is_empty()).then_some(occurrences)
}

/// One inline type hint: `label` (": TYPE") rendered after `offset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub offset: TextSize,
    pub label: String,
}

/// Type hints for the file's plain variable bindings (replacement forms
/// update an existing binding hinted at its own definition; annotated
/// bindings already show their type). A function type is hinted whenever it
/// contains no `Unknown` — its variables generalize into binder names — while
/// any other type must be fully concrete, so partially-inferred values show
/// nothing rather than noise.
pub fn inlay_hints(db: &dyn Db, file: SourceFile, viewport: Option<TextRange>) -> Vec<InlayHint> {
    let mut hints = Vec::new();
    for item in item_tree(db, file) {
        let Some(node) = item_node(db, item) else {
            continue;
        };
        let item_offset = node.text_range().start();
        if let Some(viewport) = viewport {
            let range = node.text_range();
            if range.end() < viewport.start() || viewport.end() < range.start() {
                continue;
            }
        }
        let Some(hir) = item_hir(db, item) else {
            continue;
        };
        let Some(check) = item_check(db, item) else {
            continue;
        };
        // A refused (form-mixing) annotation drops its whole payload, so the
        // binding is effectively unannotated and still hints.
        let annotated = item_annotation_syntax(db, item).is_some_and(|annotation| {
            semantics::annotations::block_form_violation(&annotation.syntax_node()).is_none()
        });

        for (index, expression) in hir.expressions.iter().enumerate() {
            let ExpressionKind::Assign { target, value, .. } = &expression.kind else {
                continue;
            };
            let target_expression = hir.expression(*target);
            if !matches!(target_expression.kind, ExpressionKind::NameRef(_)) {
                continue;
            }
            let id = ExprId(index as u32);
            let is_root = hir.root == Some(id);
            // The item's annotation binds its top-level statement; nested
            // bindings are unannotated either way.
            if is_root && annotated {
                continue;
            }

            let mut renderer = TypeRenderer::default();
            let label = if is_root {
                let Some(scheme) = &check.scheme else {
                    continue;
                };
                if !scheme_is_hintable(db, scheme) {
                    continue;
                }
                renderer.render_scheme(db, scheme)
            } else {
                let Some(ty) = check
                    .expression_types
                    .get(&id)
                    .or_else(|| check.expression_types.get(value))
                else {
                    continue;
                };
                if !is_hintable(db, *ty, matches!(ty.kind(db), TyKind::Function(_))) {
                    continue;
                }
                renderer.render(db, *ty)
            };
            hints.push(InlayHint {
                offset: target_expression.range.end() + item_offset,
                label: format!(": {label}"),
            });
        }
    }
    hints.sort_by_key(|hint| hint.offset);
    hints
}

/// A rendered call signature: the label, each parameter's byte span inside
/// it, and the parameter the cursor's argument targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    pub label: String,
    pub parameters: Vec<TextRange>,
    pub active_parameter: Option<usize>,
}

/// The inferred signature of the call under the cursor. The active parameter
/// follows R's argument matching: a named argument consumes the parameter it
/// names, a positional argument fills the first open positionally-fillable
/// slot (parameters after `...` match by name only).
pub fn signature_help(db: &dyn Db, file: SourceFile, offset: TextSize) -> Option<SignatureHelp> {
    let position = position_in_item(db, file, offset)?;
    let hir = item_hir(db, position.item)?;
    let check = item_check(db, position.item)?;

    // Containing calls, smallest first; the innermost whose callee has a
    // function type wins, so a cursor inside `list(...)` inside `f(...)`
    // still shows `f`'s signature (call-site builtins have none).
    let mut calls: Vec<(TextRange, ExprId)> = hir
        .expressions
        .iter()
        .enumerate()
        .filter_map(|(index, expression)| match &expression.kind {
            ExpressionKind::Call { .. }
                if !expression.range.is_empty()
                    && expression.range.start() <= position.relative
                    && position.relative <= expression.range.end() =>
            {
                Some((expression.range, ExprId(index as u32)))
            }
            _ => None,
        })
        .collect();
    calls.sort_by_key(|(range, id)| (range.len(), range.start(), id.0));

    let (function, arguments) = calls.iter().find_map(|(_, id)| {
        let ExpressionKind::Call { callee, arguments } = &hir.expression(*id).kind else {
            return None;
        };
        let callee_ty = *check.expression_types.get(callee)?;
        match callee_ty.kind(db) {
            TyKind::Function(function) => Some((function.clone(), arguments)),
            _ => None,
        }
    })?;
    let function = &function;

    let rendered = render_signature(db, function);
    let active_parameter = active_parameter(
        db,
        function,
        rendered.1.len(),
        arguments,
        &hir,
        position.relative,
    );
    Some(SignatureHelp {
        label: rendered.0,
        parameters: rendered.1,
        active_parameter,
    })
}

/// The candidate cap; the result marks itself incomplete past it so clients
/// re-query as the prefix narrows instead of filtering a truncated list.
pub const COMPLETION_LIMIT: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionKind {
    Keyword,
    Variable,
    Function,
    Field,
}

/// Where an item came from; the variant order is the ranking order among
/// items of equal match quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompletionSource {
    Keyword,
    Local,
    Global,
    Stdlib,
    Field,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub source: CompletionSource,
    /// The rendered scheme for standard-library entries.
    pub detail: Option<String>,
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionResult {
    pub items: Vec<CompletionItem>,
    pub is_incomplete: bool,
}

const RESERVED_WORDS: &[&str] = &[
    "if",
    "else",
    "repeat",
    "while",
    "function",
    "for",
    "in",
    "next",
    "break",
    "TRUE",
    "FALSE",
    "NULL",
    "Inf",
    "NaN",
    "NA",
    "NA_integer_",
    "NA_real_",
    "NA_complex_",
    "NA_character_",
];

pub fn completion(
    db: &dyn Db,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
) -> Option<CompletionResult> {
    let text = file.text(db);
    let (context, query) = completion_context(text, offset)?;

    // A cursor inside a `#:` annotation completes type names, not R values.
    {
        let parse = semantics::parse(db, file);
        if parse.syntax_node().descendants().any(|node| {
            node.kind() == syntax::SyntaxKind::ANNOTATION
                && node.text_range().start() <= offset
                && offset <= node.text_range().end()
        }) {
            return annotation_completion(db, files, file, &query);
        }
        // A cursor inside a string completes typed record fields when the
        // string subscripts a record (`x[["…"]]`) and is otherwise silent —
        // R value names never resolve inside string content.
        if let Some(string_token) = parse
            .syntax_node()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .find(|token| {
                token.kind() == syntax::SyntaxKind::STRING
                    && token.text_range().start() < offset
                    && offset < token.text_range().end()
            })
        {
            return string_subscript_completion(db, file, offset, &string_token);
        }
    }

    let items = match context {
        CompletionContext::Field => spelled_completions(
            db,
            files,
            &query,
            syntax::SyntaxKind::AT_EXPR,
            CompletionSource::Field,
        ),
        CompletionContext::Item => dollar_completions(db, files, file, offset, &query),
        CompletionContext::Namespace { package } => {
            namespace_completions(db, files, &package, &query)
        }
        CompletionContext::MaybeNamespace => return None,
        CompletionContext::Default => {
            let mut items = Vec::new();
            // Keywords are a small fixed set, prefix-completed: no one
            // searches for `function` by typing `con`.
            for keyword in RESERVED_WORDS {
                if prefix_under_case(keyword, &query, query_is_case_sensitive(&query)) {
                    items.push(CompletionItem {
                        label: (*keyword).to_owned(),
                        kind: CompletionKind::Keyword,
                        source: CompletionSource::Keyword,
                        detail: None,
                        documentation: None,
                    });
                }
            }
            if let Some(position) = position_in_item(db, file, offset)
                && let Some(naming) = item_naming(db, position.item)
            {
                for info in naming.bindings.values() {
                    if info.kind == BindingKind::TopLevel {
                        continue;
                    }
                    if search_match(&info.name, &query).is_some() {
                        items.push(CompletionItem {
                            label: info.name.clone(),
                            kind: CompletionKind::Variable,
                            source: CompletionSource::Local,
                            detail: None,
                            documentation: None,
                        });
                    }
                }
            }
            for &project_file in files.files(db) {
                for item in item_tree(db, project_file) {
                    let kind = match *item.kind(db) {
                        ItemKind::Function => CompletionKind::Function,
                        ItemKind::Value => CompletionKind::Variable,
                        ItemKind::Statement => continue,
                    };
                    let Some(name) = item.name(db).clone() else {
                        continue;
                    };
                    if search_match(&name, &query).is_some() {
                        items.push(CompletionItem {
                            label: name,
                            kind,
                            source: CompletionSource::Global,
                            detail: None,
                            documentation: None,
                        });
                    }
                }
            }
            // The standard-library corpus, with each stub's scheme as the
            // detail. A project global of the same name outranks its stub at
            // the deduplication step, mirroring how resolution shadows.
            if let Some(library) = semantics::stubs::stubs(db) {
                for (name, schemes) in &library.schemes {
                    if search_match(name, &query).is_none() {
                        continue;
                    }
                    let Some(scheme) = schemes.first() else {
                        continue;
                    };
                    let mut renderer = TypeRenderer::default();
                    let namespace = library
                        .exports_by_namespace
                        .iter()
                        .find(|(_, names)| names.contains(name))
                        .map(|(namespace, _)| namespace.clone());
                    items.push(CompletionItem {
                        label: name.clone(),
                        kind: match scheme.body.kind(db) {
                            TyKind::Function(_) => CompletionKind::Function,
                            _ => CompletionKind::Variable,
                        },
                        source: CompletionSource::Stdlib,
                        detail: Some(renderer.render_scheme(db, scheme)),
                        documentation: namespace
                            .map(|namespace| format!("From the `{namespace}` package.")),
                    });
                }
            }
            items
        }
    };

    finish_completions(items, &query)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeActionKind {
    /// Replace `@if-unknown` with `@trust` — the directive's one upgrade
    /// path once the value's type IS determined.
    IfUnknownToTrust,
    RemoveUnusedAssignment,
    /// Prefix the written name with `.` to keep it (dot-names are exempt
    /// from the unused check).
    PrefixDot,
    InsertInferredAnnotation,
    /// The whole-file source action covering every eligible binding.
    AddMissingAnnotations,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub range: TextRange,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeAction {
    pub title: String,
    pub kind: CodeActionKind,
    pub edits: Vec<TextEdit>,
}

/// The static quickfixes and source actions for a document range: rewriting
/// `@if-unknown` to `@trust`, removing or dot-prefixing an unused
/// assignment, and inserting an inferred `#:` annotation above an
/// unannotated binding (the text the inlay hint shows). All edits are
/// computed eagerly — no resolve round-trip.
pub fn code_actions(db: &dyn Db, file: SourceFile, viewport: TextRange) -> Vec<CodeAction> {
    let text = file.text(db);
    let mut actions = Vec::new();

    let parse = semantics::parse(db, file);
    for annotation in parse
        .syntax_node()
        .descendants()
        .filter(|node| node.kind() == syntax::SyntaxKind::ANNOTATION)
    {
        if !ranges_overlap(annotation.text_range(), viewport) {
            continue;
        }
        for directive in annotation
            .descendants()
            .filter(|node| node.kind() == syntax::SyntaxKind::ANNOTATION_DIRECTIVE)
        {
            if let Some((name, range)) = directive_name_range(&directive)
                && name == "if-unknown"
            {
                actions.push(CodeAction {
                    title: "Replace `@if-unknown` with `@trust`".to_owned(),
                    kind: CodeActionKind::IfUnknownToTrust,
                    edits: vec![TextEdit {
                        range,
                        replacement: "@trust".to_owned(),
                    }],
                });
            }
        }
    }

    let mut all_annotation_edits = Vec::new();
    for item in item_tree(db, file) {
        let Some(node) = item_node(db, item) else {
            continue;
        };
        let item_offset = node.text_range().start();
        let Some(naming) = item_naming(db, item) else {
            continue;
        };
        let Some(hir) = item_hir(db, item) else {
            continue;
        };

        for unused in &naming.unused_assignments {
            let target_range = unused.range + item_offset;
            if !ranges_overlap(target_range, viewport) {
                continue;
            }
            let Some(assign_range) = hir.expressions.iter().find_map(|expression| {
                let ExpressionKind::Assign { target, .. } = &expression.kind else {
                    return None;
                };
                (hir.expression(*target).range == unused.range || expression.range == unused.range)
                    .then_some(trim_trailing_trivia(text, expression.range + item_offset))
            }) else {
                continue;
            };
            actions.push(CodeAction {
                title: format!("Remove unused assignment of `{}`", unused.name),
                kind: CodeActionKind::RemoveUnusedAssignment,
                edits: vec![TextEdit {
                    range: removal_range(text, assign_range),
                    replacement: String::new(),
                }],
            });
            actions.push(CodeAction {
                title: format!("Prefix `{}` with `.` to keep it", unused.name),
                kind: CodeActionKind::PrefixDot,
                edits: vec![TextEdit {
                    range: TextRange::empty(target_range.start()),
                    replacement: ".".to_owned(),
                }],
            });
        }

        let Some(check) = item_check(db, item) else {
            continue;
        };
        let annotated = item_annotation_syntax(db, item).is_some_and(|annotation| {
            semantics::annotations::block_form_violation(&annotation.syntax_node()).is_none()
        });
        for (index, expression) in hir.expressions.iter().enumerate() {
            let ExpressionKind::Assign { target, value, .. } = &expression.kind else {
                continue;
            };
            let target_expression = hir.expression(*target);
            let ExpressionKind::NameRef(name) = &target_expression.kind else {
                continue;
            };
            let id = ExprId(index as u32);
            let is_root = hir.root == Some(id);
            if is_root && annotated {
                continue;
            }
            let start = usize::from(expression.range.start() + item_offset);
            let line_start = text[..start.min(text.len())]
                .rfind('\n')
                .map_or(0, |at| at + 1);
            if !text[line_start..start].trim().is_empty() {
                continue;
            }

            let mut renderer = TypeRenderer::default();
            let rendered = if is_root {
                let Some(scheme) = &check.scheme else {
                    continue;
                };
                if !scheme_is_hintable(db, scheme) {
                    continue;
                }
                renderer.render_scheme(db, scheme)
            } else {
                let Some(ty) = check
                    .expression_types
                    .get(&id)
                    .or_else(|| check.expression_types.get(value))
                else {
                    continue;
                };
                if !is_hintable(db, *ty, matches!(ty.kind(db), TyKind::Function(_))) {
                    continue;
                }
                renderer.render(db, *ty)
            };
            let indentation = &text[line_start..start];
            let edit = TextEdit {
                range: TextRange::empty(TextSize::from(line_start as u32)),
                replacement: format!("{indentation}#: {rendered}\n"),
            };
            all_annotation_edits.push(edit.clone());
            if ranges_overlap(expression.range + item_offset, viewport) {
                actions.push(CodeAction {
                    title: format!("Add inferred type annotation for `{name}`"),
                    kind: CodeActionKind::InsertInferredAnnotation,
                    edits: vec![edit],
                });
            }
        }
    }
    if !all_annotation_edits.is_empty() {
        actions.push(CodeAction {
            title: "Add inferred type annotations for the whole file".to_owned(),
            kind: CodeActionKind::AddMissingAnnotations,
            edits: all_annotation_edits,
        });
    }

    actions
}

fn ranges_overlap(left: TextRange, right: TextRange) -> bool {
    left.start() <= right.end() && right.start() <= left.end()
}

/// The range removing an assignment deletes: the whole line(s) — trailing
/// newline included — when the assignment is the only content on them,
/// otherwise exactly the assignment's own range.
fn removal_range(text: &str, range: TextRange) -> TextRange {
    let start = usize::from(range.start()).min(text.len());
    let end = usize::from(range.end()).min(text.len());
    let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |at| end + at);
    let alone = text[line_start..start].trim().is_empty() && text[end..line_end].trim().is_empty();
    if !alone {
        return range;
    }
    let with_newline = if line_end < text.len() {
        line_end + 1
    } else {
        line_end
    };
    TextRange::new(
        TextSize::from(line_start as u32),
        TextSize::from(with_newline as u32),
    )
}

/// The `@` + joined directive name span (`@if-unknown` spans the `@` through
/// `unknown`), for rewrites that replace the directive keyword.
fn directive_name_range(directive: &syntax::SyntaxNode) -> Option<(String, TextRange)> {
    let name = directive_name(directive)?;
    let at = directive
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == syntax::SyntaxKind::AT)?;
    let start = at.text_range().start();
    let end = start + TextSize::from(1 + name.len() as u32);
    Some((name, TextRange::new(start, end)))
}

/// Goto type definition: when the expression under the cursor has a named
/// (nominal or alias) type, the `@type`/`@alias` declaration of that name.
pub fn type_definition(
    db: &dyn Db,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
) -> Option<NavigationTarget> {
    let position = position_in_item(db, file, offset)?;
    let check = item_check(db, position.item)?;
    let (_, ty) = position
        .expressions_at()
        .into_iter()
        .find_map(|id| check.expression_types.get(&id).map(|ty| (id, *ty)))?;
    let TyKind::Named(name, _) = ty.kind(db) else {
        return None;
    };
    let name = name.text(db).to_owned();
    type_name_occurrences(db, files, &name)
        .into_iter()
        .rfind(|occurrence| occurrence.is_declaration)
        .map(|occurrence| NavigationTarget {
            file: occurrence.file,
            range: occurrence.range,
        })
}

/// Document symbols: the file's named top-level definitions with their
/// absolute name ranges, in source order.
pub fn document_symbols(db: &dyn Db, file: SourceFile) -> Vec<(String, NavigationTarget)> {
    let mut symbols = Vec::new();
    for item in item_tree(db, file) {
        if !matches!(*item.kind(db), ItemKind::Function | ItemKind::Value) {
            continue;
        }
        let Some(name) = item.name(db).clone() else {
            continue;
        };
        let Some(node) = item_node(db, item) else {
            continue;
        };
        let range = definition_name_range(&node).unwrap_or_else(|| node.text_range());
        symbols.push((name, NavigationTarget { file, range }));
    }
    symbols
}

/// Workspace symbols: every file's named definitions matched and ranked by
/// the shared smart-case matcher, capped like completion.
pub fn workspace_symbols(
    db: &dyn Db,
    files: ProjectFiles,
    query: &str,
) -> Vec<(String, NavigationTarget)> {
    let mut symbols: Vec<(MatchScore, String, NavigationTarget)> = Vec::new();
    for &file in files.files(db) {
        for (name, target) in document_symbols(db, file) {
            if let Some(score) = search_match(&name, query) {
                symbols.push((score, name, target));
            }
        }
    }
    symbols.sort_by(|left, right| {
        (left.0, left.1.to_lowercase(), &left.1).cmp(&(right.0, right.1.to_lowercase(), &right.1))
    });
    symbols.truncate(COMPLETION_LIMIT);
    symbols
        .into_iter()
        .map(|(_, name, target)| (name, target))
        .collect()
}

// ---- annotation type names ----

/// A type-name token under the cursor inside a `#:` annotation.
struct AnnotationTypeCursor {
    name: String,
    range: TextRange,
    /// Whether the token can resolve to a project `@type`/`@alias`
    /// declaration: binder-shadowed names, binder declarations, and `fn`
    /// cannot — they still hover, showing themselves.
    navigable: bool,
}

/// The type-name token at the cursor: a `TYPE_REF`'s identifier, a
/// `TYPE_APPLY`'s name, the declared name of an `@type`/`@alias` directive,
/// a `<T>` binder name, or the `fn` keyword of a function type.
fn annotation_type_at(
    db: &dyn Db,
    file: SourceFile,
    offset: TextSize,
) -> Option<AnnotationTypeCursor> {
    let parse = semantics::parse(db, file);
    let root = parse.syntax_node();
    let annotation = root.descendants().find(|node| {
        node.kind() == syntax::SyntaxKind::ANNOTATION
            && node.text_range().start() <= offset
            && offset <= node.text_range().end()
    })?;

    let token = annotation
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| {
            matches!(
                token.kind(),
                syntax::SyntaxKind::IDENT | syntax::SyntaxKind::NULL_KW
            ) && token.text_range().start() <= offset
                && offset <= token.text_range().end()
        })?;
    let name = token.text().to_string();
    let range = token.text_range();

    let parent = token.parent()?;
    let (is_type_position, resolvable) = match parent.kind() {
        syntax::SyntaxKind::TYPE_REF => (true, token.kind() == syntax::SyntaxKind::IDENT),
        syntax::SyntaxKind::NAME => match parent.parent().map(|grandparent| grandparent.kind()) {
            Some(syntax::SyntaxKind::TYPE_APPLY) => (true, true),
            Some(syntax::SyntaxKind::ANNOTATION_DIRECTIVE) => (
                parent.parent().is_some_and(|grandparent| {
                    matches!(
                        directive_name(&grandparent).as_deref(),
                        Some("type" | "alias")
                    )
                }),
                true,
            ),
            // Structural keywords (`fn`, `list`, a `<T>` binder) hover as
            // themselves.
            Some(other) if type_kind(other) => (true, false),
            _ => (false, false),
        },
        // `NULL` and other bare tokens directly inside a type node.
        other if type_kind(other) => (true, false),
        _ => (false, false),
    };
    if !is_type_position {
        return None;
    }
    let navigable = resolvable && !binder_names(&annotation).contains(&name);
    Some(AnnotationTypeCursor {
        name,
        range,
        navigable,
    })
}

// ---- S4 navigation ----
//
// S4 class/generic/method names are written as string literals inside
// `setClass` / `setGeneric` / `setMethod` / `new` calls, invisible to the
// naming analysis, so they are recovered structurally from the tree — one
// recognizer shared by goto-definition, references, and rename.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum S4Kind {
    /// Declared by `setClass`, referenced by `setMethod` signatures and `new`.
    Class,
    /// Declared by `setGeneric`, referenced by `setMethod`'s function name.
    Generic,
}

struct S4Occurrence {
    name: String,
    kind: S4Kind,
    /// The string CONTENT range (inside the quotes), absolute.
    range: TextRange,
    is_declaration: bool,
}

/// Every S4 name occurrence in one file, in tree order.
fn s4_occurrences_in(db: &dyn Db, file: SourceFile) -> Vec<S4Occurrence> {
    let parse = semantics::parse(db, file);
    let mut occurrences = Vec::new();
    for call in parse
        .syntax_node()
        .descendants()
        .filter(|node| node.kind() == syntax::SyntaxKind::CALL_EXPR)
    {
        let Some(callee) = s4_callee_name(&call) else {
            continue;
        };
        let Some(arguments) = call
            .children()
            .find(|child| child.kind() == syntax::SyntaxKind::ARGUMENT_LIST)
        else {
            continue;
        };
        match callee.as_str() {
            "setClass" => push_s4_string(
                s4_argument(&arguments, "Class", 0),
                S4Kind::Class,
                true,
                &mut occurrences,
            ),
            "setGeneric" => push_s4_string(
                s4_argument(&arguments, "name", 0),
                S4Kind::Generic,
                true,
                &mut occurrences,
            ),
            "setMethod" => {
                push_s4_string(
                    s4_argument(&arguments, "f", 0),
                    S4Kind::Generic,
                    false,
                    &mut occurrences,
                );
                // The signature is a class name or a `c(...)` of class names.
                if let Some(signature) = s4_argument(&arguments, "signature", 1) {
                    for class_string in s4_signature_strings(&signature) {
                        push_s4_string(Some(class_string), S4Kind::Class, false, &mut occurrences);
                    }
                }
            }
            "new" => push_s4_string(
                s4_argument(&arguments, "Class", 0),
                S4Kind::Class,
                false,
                &mut occurrences,
            ),
            _ => {}
        }
    }
    occurrences
}

/// The bare callee name of a call: `f(...)` or `pkg::f(...)`.
fn s4_callee_name(call: &syntax::SyntaxNode) -> Option<String> {
    let callee = call.children().next()?;
    match callee.kind() {
        syntax::SyntaxKind::NAME => Some(callee.text().to_string()),
        syntax::SyntaxKind::NAMESPACE_EXPR => callee
            .children()
            .filter(|child| child.kind() == syntax::SyntaxKind::NAME)
            .last()
            .map(|name| name.text().to_string()),
        _ => None,
    }
}

/// Resolves a call argument's value by name, falling back to the positional
/// slot at `index` when unnamed — R's own argument-matching shape.
fn s4_argument(
    arguments: &syntax::SyntaxNode,
    name: &str,
    index: usize,
) -> Option<syntax::SyntaxNode> {
    let all: Vec<syntax::SyntaxNode> = arguments
        .children()
        .filter(|child| child.kind() == syntax::SyntaxKind::ARGUMENT)
        .collect();
    for argument in &all {
        if let Some(argument_name) = argument
            .children()
            .find(|child| child.kind() == syntax::SyntaxKind::NAME)
            && argument_name.text() == name
        {
            return argument
                .children()
                .find(|child| child.kind() != syntax::SyntaxKind::NAME);
        }
    }
    all.get(index)
        .filter(|argument| {
            // Positional: no `name =` tag. A NAME child followed by EQ is
            // the tag; a bare NAME child is the value itself.
            !argument
                .children_with_tokens()
                .any(|element| element.kind() == syntax::SyntaxKind::EQ)
        })
        .and_then(|argument| argument.children().next())
}

/// The class-name strings of a `setMethod` signature: a single string, or
/// the string elements of a `c(...)` vector.
fn s4_signature_strings(signature: &syntax::SyntaxNode) -> Vec<syntax::SyntaxNode> {
    if is_string_literal(signature) {
        return vec![signature.clone()];
    }
    if signature.kind() == syntax::SyntaxKind::CALL_EXPR
        && let Some(arguments) = signature
            .children()
            .find(|child| child.kind() == syntax::SyntaxKind::ARGUMENT_LIST)
    {
        return arguments
            .children()
            .filter(|child| child.kind() == syntax::SyntaxKind::ARGUMENT)
            .filter_map(|argument| argument.children().next())
            .filter(is_string_literal)
            .collect();
    }
    Vec::new()
}

fn is_string_literal(node: &syntax::SyntaxNode) -> bool {
    node.kind() == syntax::SyntaxKind::LITERAL
        && node
            .children_with_tokens()
            .any(|element| element.kind() == syntax::SyntaxKind::STRING)
}

fn push_s4_string(
    node: Option<syntax::SyntaxNode>,
    kind: S4Kind,
    is_declaration: bool,
    out: &mut Vec<S4Occurrence>,
) {
    let Some(node) = node else {
        return;
    };
    let Some(token) = node
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == syntax::SyntaxKind::STRING)
    else {
        return;
    };
    // The content between plain quotes; raw strings never carry S4 names.
    let text = token.text();
    if text.len() < 2 || !(text.starts_with('"') || text.starts_with('\'')) {
        return;
    }
    let name = text[1..text.len() - 1].to_string();
    if name.is_empty() {
        return;
    }
    let range = token.text_range();
    out.push(S4Occurrence {
        name,
        kind,
        range: TextRange::new(
            range.start() + TextSize::from(1),
            range.end() - TextSize::from(1),
        ),
        is_declaration,
    });
}

/// Whether a node kind belongs to the annotation type grammar.
fn type_kind(kind: syntax::SyntaxKind) -> bool {
    matches!(
        kind,
        syntax::SyntaxKind::TYPE_REF
            | syntax::SyntaxKind::TYPE_APPLY
            | syntax::SyntaxKind::TYPE_VECTOR
            | syntax::SyntaxKind::TYPE_UNION
            | syntax::SyntaxKind::TYPE_FUNCTION
            | syntax::SyntaxKind::TYPE_RECORD
            | syntax::SyntaxKind::TYPE_TUPLE
            | syntax::SyntaxKind::TYPE_LIST
            | syntax::SyntaxKind::TYPE_PAREN
            | syntax::SyntaxKind::TYPE_BINDER_LIST
            | syntax::SyntaxKind::TYPE_BINDER
            | syntax::SyntaxKind::TYPE_PARAMETER_LIST
            | syntax::SyntaxKind::TYPE_ARG_LIST
    )
}

/// The directive's joined name (`type`, `alias`, `if-unknown`, …): adjacent
/// name-ish tokens right after the `@`.
fn directive_name(directive: &syntax::SyntaxNode) -> Option<String> {
    let mut name = String::new();
    let mut end: Option<TextSize> = None;
    for element in directive.children_with_tokens() {
        let Some(token) = element.as_token() else {
            break;
        };
        match token.kind() {
            syntax::SyntaxKind::AT => {
                end = Some(token.text_range().end());
            }
            syntax::SyntaxKind::WHITESPACE | syntax::SyntaxKind::NEWLINE => break,
            // Only tokens touching the previous one join the name
            // (`@if-unknown` lexes as `if` `-` `unknown`).
            _ if end.is_some() && Some(token.text_range().start()) == end => {
                name.push_str(token.text());
                end = Some(token.text_range().end());
            }
            _ => break,
        }
    }
    (!name.is_empty()).then_some(name)
}

/// The `<T>` binder names declared anywhere in one annotation region.
fn binder_names(annotation: &syntax::SyntaxNode) -> Vec<String> {
    annotation
        .descendants()
        .filter(|node| node.kind() == syntax::SyntaxKind::TYPE_BINDER)
        .filter_map(|binder| {
            binder
                .descendants_with_tokens()
                .filter_map(|element| element.into_token())
                .find(|token| token.kind() == syntax::SyntaxKind::IDENT)
                .map(|token| token.text().to_string())
        })
        .collect()
}

fn annotation_type_hover(
    db: &dyn Db,
    file: SourceFile,
    cursor: &AnnotationTypeCursor,
) -> Option<Hover> {
    // Resolve against the file's own declarations (matching how the
    // checker's winner table folds files). A builtin or undeclared name has
    // no definition to expand — show the name itself, confirming what the
    // cursor is on.
    let definition = cursor
        .navigable
        .then(|| {
            semantics::file_type_definitions(db, file)
                .into_iter()
                .find(|definition| definition.name.text(db) == cursor.name)
        })
        .flatten();
    let line = match definition {
        Some(definition) => {
            let mut renderer = TypeRenderer::default();
            let parameters = if definition.parameters.is_empty() {
                String::new()
            } else {
                let names: Vec<String> = definition
                    .parameters
                    .iter()
                    .map(|parameter| parameter.text(db).to_owned())
                    .collect();
                format!("<{}>", names.join(", "))
            };
            let keyword = if definition.alias { "@alias" } else { "@type" };
            format!(
                "{keyword} {}{parameters} {{{}}}",
                cursor.name,
                renderer.render(db, definition.body)
            )
        }
        None => cursor.name.clone(),
    };
    Some(Hover {
        range: cursor.range,
        lines: vec![line],
    })
}

/// The declaration summary for a cursor anywhere inside an `@type`/`@alias`
/// declaration (the `@`, the keyword, the braces), spanning the
/// declaration's own lines — a stitched block may hold several.
fn annotation_definition_hover(db: &dyn Db, file: SourceFile, offset: TextSize) -> Option<Hover> {
    let parse = semantics::parse(db, file);
    let root = parse.syntax_node();
    let annotation = root.descendants().find(|node| {
        node.kind() == syntax::SyntaxKind::ANNOTATION
            && node.text_range().start() <= offset
            && offset <= node.text_range().end()
    })?;
    for directive in annotation.descendants().filter(|node| {
        node.kind() == syntax::SyntaxKind::ANNOTATION_DIRECTIVE
            && matches!(directive_name(node).as_deref(), Some("type" | "alias"))
    }) {
        // The declaration's display range starts at its line's `#:` marker.
        let start = annotation
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| {
                token.kind() == syntax::SyntaxKind::ANNOTATION_MARKER
                    && token.text_range().start() <= directive.text_range().start()
            })
            .map(|token| token.text_range().start())
            .last()
            .unwrap_or_else(|| directive.text_range().start());
        let range = TextRange::new(start, directive.text_range().end());
        if !(range.start() <= offset && offset <= range.end()) {
            continue;
        }
        let Some(name) = directive
            .children()
            .find(|child| child.kind() == syntax::SyntaxKind::NAME)
            .map(|name| name.text().to_string())
        else {
            continue;
        };
        let cursor = AnnotationTypeCursor {
            name,
            range,
            navigable: true,
        };
        return annotation_type_hover(db, file, &cursor);
    }
    None
}

/// Every occurrence of a type name across the project's annotations:
/// declarations (`@type`/`@alias` names) and uses (`TYPE_REF` identifiers,
/// `TYPE_APPLY` names), skipping annotations where a binder shadows it.
fn type_name_occurrences(db: &dyn Db, files: ProjectFiles, name: &str) -> Vec<Occurrence> {
    let mut result = Vec::new();
    for &file in files.files(db) {
        let parse = semantics::parse(db, file);
        for annotation in parse
            .syntax_node()
            .descendants()
            .filter(|node| node.kind() == syntax::SyntaxKind::ANNOTATION)
        {
            let shadowed = binder_names(&annotation)
                .iter()
                .any(|binder| binder == name);
            for node in annotation.descendants() {
                let (token, is_declaration) = match node.kind() {
                    syntax::SyntaxKind::TYPE_REF => {
                        let Some(token) = node
                            .children_with_tokens()
                            .filter_map(|element| element.into_token())
                            .find(|token| token.kind() == syntax::SyntaxKind::IDENT)
                        else {
                            continue;
                        };
                        (token, false)
                    }
                    syntax::SyntaxKind::NAME => {
                        let Some(parent) = node.parent() else {
                            continue;
                        };
                        let is_declaration = match parent.kind() {
                            syntax::SyntaxKind::TYPE_APPLY => false,
                            syntax::SyntaxKind::ANNOTATION_DIRECTIVE => {
                                if !matches!(
                                    directive_name(&parent).as_deref(),
                                    Some("type" | "alias")
                                ) {
                                    continue;
                                }
                                true
                            }
                            _ => continue,
                        };
                        let Some(token) = node
                            .children_with_tokens()
                            .filter_map(|element| element.into_token())
                            .find(|token| token.kind() == syntax::SyntaxKind::IDENT)
                        else {
                            continue;
                        };
                        (token, is_declaration)
                    }
                    _ => continue,
                };
                if token.text() != name || (shadowed && !is_declaration) {
                    continue;
                }
                result.push(Occurrence {
                    file,
                    range: token.text_range(),
                    is_declaration,
                });
            }
        }
    }
    result
}

/// Record-field completion inside the string of `x[["…"]]`: the subscripted
/// value's typed fields matched against the content typed before the cursor.
fn string_subscript_completion(
    db: &dyn Db,
    file: SourceFile,
    offset: TextSize,
    string_token: &syntax::SyntaxToken,
) -> Option<CompletionResult> {
    let position = position_in_item(db, file, offset)?;
    let hir = item_hir(db, position.item)?;
    let check = item_check(db, position.item)?;

    let string_range = string_token.text_range() - position.item_offset;
    let target = hir
        .expressions
        .iter()
        .filter_map(|expression| match &expression.kind {
            ExpressionKind::Index {
                double: true,
                target,
                arguments,
            } if arguments.iter().any(|argument| {
                argument
                    .value
                    .is_some_and(|value| hir.expression(value).range == string_range)
            }) =>
            {
                Some(*target)
            }
            _ => None,
        })
        .next()?;
    let ty = *check.expression_types.get(&target)?;
    let TyKind::Record(fields) = ty.kind(db) else {
        return None;
    };

    // The query is the content typed before the cursor, past the open quote.
    let text = string_token.text();
    let typed = usize::from(offset - string_token.text_range().start());
    let query = text.get(1..typed).unwrap_or_default();

    let mut renderer = TypeRenderer::default();
    let items: Vec<CompletionItem> = fields
        .iter()
        .filter(|field| search_match(field.name.text(db), query).is_some())
        .map(|field| CompletionItem {
            label: field.name.text(db).to_owned(),
            kind: CompletionKind::Field,
            source: CompletionSource::Field,
            detail: Some(renderer.render(db, field.ty)),
            documentation: None,
        })
        .collect();
    finish_completions(items, query)
}

/// Type-name completion inside a `#:` annotation: the primitive vocabulary
/// plus the project's `@type`/`@alias` declarations.
fn annotation_completion(
    db: &dyn Db,
    files: ProjectFiles,
    file: SourceFile,
    query: &str,
) -> Option<CompletionResult> {
    const PRIMITIVES: &[&str] = &[
        "logical",
        "integer",
        "double",
        "complex",
        "character",
        "raw",
        "list",
        "fn",
        "Any",
        "Unknown",
        "NULL",
    ];
    let mut items = Vec::new();
    for primitive in PRIMITIVES {
        if search_match(primitive, query).is_some() {
            items.push(CompletionItem {
                label: (*primitive).to_owned(),
                kind: CompletionKind::Keyword,
                source: CompletionSource::Keyword,
                detail: None,
                documentation: None,
            });
        }
    }
    // The project table covers package files only; a script's own
    // declarations complete too.
    let mut definitions: Vec<(String, semantics::annotations::NamedDefinition<'_>)> =
        semantics::project_type_definitions(db, files)
            .iter()
            .map(|(name, definition)| (name.text(db).to_owned(), definition.clone()))
            .collect();
    for definition in semantics::file_type_definitions(db, file) {
        let label = definition.name.text(db).to_owned();
        if !definitions.iter().any(|(name, _)| *name == label) {
            definitions.push((label, definition));
        }
    }
    for (label, definition) in definitions {
        if search_match(&label, query).is_none() {
            continue;
        }
        let mut renderer = TypeRenderer::default();
        items.push(CompletionItem {
            label,
            kind: CompletionKind::Variable,
            source: CompletionSource::Global,
            detail: Some(renderer.render(db, definition.body)),
            documentation: None,
        });
    }
    finish_completions(items, query)
}

// ---- completion internals ----

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompletionContext {
    Default,
    /// After `@`: S4 slot names.
    Field,
    /// After `$`: record fields of the extracted value.
    Item,
    /// After `::` / `:::`: the package's exports.
    Namespace {
        package: String,
    },
    /// A bare trailing `:` — the range operator or half a `::`; undecided,
    /// so stay silent.
    MaybeNamespace,
}

/// The completion context and query: the identifier chars immediately before
/// the cursor, and the operator (if any) they follow. Works on raw text so
/// completion still fires mid-edit inside broken code.
fn completion_context(text: &str, offset: TextSize) -> Option<(CompletionContext, String)> {
    let at = usize::from(offset).min(text.len());
    let line_start = text[..at].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &text[line_start..at];

    let mut context = CompletionContext::Default;
    let mut query = String::new();
    let mut word_start_of_previous: Option<usize> = None;
    let mut current_word_start = 0usize;
    let mut previous_char: Option<char> = None;
    for (index, character) in prefix.char_indices() {
        if character.is_alphabetic()
            || character == '.'
            || character == '_'
            || (!query.is_empty() && character.is_numeric())
        {
            if query.is_empty() {
                current_word_start = index;
            }
            query.push(character);
            // A name after a single `:` is the range operator's operand
            // (`1:n`), not a pending namespace access.
            if context == CompletionContext::MaybeNamespace {
                context = CompletionContext::Default;
            }
        } else {
            if !query.is_empty() {
                word_start_of_previous = Some(current_word_start);
            }
            context = match character {
                '@' => CompletionContext::Field,
                '$' => CompletionContext::Item,
                ':' => {
                    if previous_char == Some(':') {
                        let package = word_start_of_previous
                            .map(|start| {
                                prefix[start..]
                                    .chars()
                                    .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
                                    .collect::<String>()
                            })
                            .unwrap_or_default();
                        CompletionContext::Namespace { package }
                    } else {
                        CompletionContext::MaybeNamespace
                    }
                }
                _ => CompletionContext::Default,
            };
            query.clear();
        }
        previous_char = Some(character);
    }
    Some((context, query))
}

/// Names spelled after the given extract operator anywhere in the project —
/// the syntactic fallback shared by `@` completion and untyped `$` targets.
fn spelled_completions(
    db: &dyn Db,
    files: ProjectFiles,
    query: &str,
    kind: syntax::SyntaxKind,
    source: CompletionSource,
) -> Vec<CompletionItem> {
    let mut labels = std::collections::BTreeSet::new();
    for &project_file in files.files(db) {
        let parse = semantics::parse(db, project_file);
        for node in parse.syntax_node().descendants() {
            if node.kind() != kind {
                continue;
            }
            // The rhs name: the LAST name child (the lhs is a child
            // expression node, so a bare NAME child is the field).
            if let Some(name) = node
                .children()
                .filter(|child| child.kind() == syntax::SyntaxKind::NAME)
                .last()
            {
                labels.insert(name.text().to_string());
            }
        }
    }
    labels
        .into_iter()
        .filter(|label| search_match(label, query).is_some())
        .map(|label| CompletionItem {
            label,
            kind: CompletionKind::Field,
            source,
            detail: None,
            documentation: None,
        })
        .collect()
}

/// `$` completion: the record fields of the target's checked type, falling
/// back to every `$name` spelled in the project when the target's type gives
/// no fields.
fn dollar_completions(
    db: &dyn Db,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
    query: &str,
) -> Vec<CompletionItem> {
    let typed = (|| {
        let position = position_in_item(db, file, offset)?;
        let hir = item_hir(db, position.item)?;
        let check = item_check(db, position.item)?;
        // The innermost field access containing the cursor; its target's
        // record fields are the candidates.
        let target = hir
            .expressions
            .iter()
            .filter_map(|expression| match &expression.kind {
                ExpressionKind::Field { target, .. }
                    if !expression.range.is_empty()
                        && expression.range.start() <= position.relative
                        && position.relative <= expression.range.end() =>
                {
                    Some((expression.range, *target))
                }
                _ => None,
            })
            .min_by_key(|(range, _)| range.len())
            .map(|(_, target)| target)?;
        let ty = *check.expression_types.get(&target)?;
        let TyKind::Record(fields) = ty.kind(db) else {
            return None;
        };
        let mut renderer = TypeRenderer::default();
        Some(
            fields
                .iter()
                .map(|field| CompletionItem {
                    label: field.name.text(db).to_owned(),
                    kind: CompletionKind::Field,
                    source: CompletionSource::Field,
                    detail: Some(renderer.render(db, field.ty)),
                    documentation: None,
                })
                .filter(|item| search_match(&item.label, query).is_some())
                .collect::<Vec<_>>(),
        )
    })();
    match typed {
        Some(items) if !items.is_empty() => items,
        _ => spelled_completions(
            db,
            files,
            query,
            syntax::SyntaxKind::DOLLAR_EXPR,
            CompletionSource::Field,
        ),
    }
}

/// `pkg::` completion: the namespace's declared exports when the stub corpus
/// knows the package, otherwise every name spelled after `::` in the project.
fn namespace_completions(
    db: &dyn Db,
    files: ProjectFiles,
    package: &str,
    query: &str,
) -> Vec<CompletionItem> {
    if let Some(library) = semantics::stubs::stubs(db)
        && let Some(exports) = library.exports_by_namespace.get(package)
    {
        let mut items: Vec<CompletionItem> = exports
            .iter()
            .filter(|name| search_match(name, query).is_some())
            .map(|name| {
                let mut renderer = TypeRenderer::default();
                let (kind, detail) = match library.schemes.get(name).and_then(|s| s.first()) {
                    Some(scheme) => (
                        match scheme.body.kind(db) {
                            TyKind::Function(_) => CompletionKind::Function,
                            _ => CompletionKind::Variable,
                        },
                        Some(renderer.render_scheme(db, scheme)),
                    ),
                    None => (CompletionKind::Variable, None),
                };
                CompletionItem {
                    label: name.clone(),
                    kind,
                    source: CompletionSource::Stdlib,
                    detail,
                    documentation: None,
                }
            })
            .collect();
        items.sort_by(|left, right| left.label.cmp(&right.label));
        return items;
    }
    spelled_completions(
        db,
        files,
        query,
        syntax::SyntaxKind::NAMESPACE_EXPR,
        CompletionSource::Global,
    )
}

/// Dedupe by label, rank by match quality then source/label, cap at
/// `COMPLETION_LIMIT`.
fn finish_completions(items: Vec<CompletionItem>, query: &str) -> Option<CompletionResult> {
    let mut seen = std::collections::BTreeSet::new();
    let mut deduplicated = Vec::new();
    for item in items {
        if seen.insert(item.label.clone()) {
            deduplicated.push(item);
        }
    }
    deduplicated.sort_by(|left, right| {
        (
            search_match(&left.label, query),
            left.source,
            left.label.to_lowercase(),
            left.label.clone(),
            left.kind,
        )
            .cmp(&(
                search_match(&right.label, query),
                right.source,
                right.label.to_lowercase(),
                right.label.clone(),
                right.kind,
            ))
    });
    let is_incomplete = deduplicated.len() > COMPLETION_LIMIT;
    deduplicated.truncate(COMPLETION_LIMIT);
    (!deduplicated.is_empty()).then_some(CompletionResult {
        items: deduplicated,
        is_incomplete,
    })
}

// ---- search matching (shared by completion and symbol search) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchScore {
    tier: u8,
    first_match_index: u32,
}

const TIER_EXACT: u8 = 0;
const TIER_PREFIX: u8 = 1;
const TIER_SUBSTRING: u8 = 2;
const TIER_SUBSEQUENCE: u8 = 3;

/// Shortest query matched as a subsequence; shorter queries fall back to
/// prefix matching so one- or two-character inputs do not surface scattered,
/// low-signal matches (mirrors rust-analyzer).
const MIN_SUBSEQUENCE_QUERY_LEN: usize = 3;

/// Subsequence matching with smart case: every query character must appear
/// in the candidate in order; case-insensitive unless the query contains an
/// uppercase character. An empty query matches everything.
pub fn search_match(candidate: &str, query: &str) -> Option<MatchScore> {
    if query.is_empty() {
        return Some(MatchScore {
            tier: TIER_PREFIX,
            first_match_index: 0,
        });
    }

    let case_sensitive = query_is_case_sensitive(query);
    let equal = |left: char, right: char| {
        if case_sensitive {
            left == right
        } else {
            left.to_lowercase().eq(right.to_lowercase())
        }
    };

    let mut query_chars = query.chars().peekable();
    let mut first_match_index = None;
    for (index, candidate_char) in candidate.chars().enumerate() {
        let Some(&query_char) = query_chars.peek() else {
            break;
        };
        if equal(candidate_char, query_char) {
            if first_match_index.is_none() {
                first_match_index = Some(index as u32);
            }
            query_chars.next();
        }
    }
    if query_chars.peek().is_some() {
        return None;
    }

    let tier = if equal_under_case(candidate, query, case_sensitive) {
        TIER_EXACT
    } else if prefix_under_case(candidate, query, case_sensitive) {
        TIER_PREFIX
    } else if substring_under_case(candidate, query, case_sensitive) {
        TIER_SUBSTRING
    } else {
        TIER_SUBSEQUENCE
    };
    if query.chars().count() < MIN_SUBSEQUENCE_QUERY_LEN && tier > TIER_PREFIX {
        return None;
    }
    Some(MatchScore {
        tier,
        first_match_index: first_match_index.unwrap_or(0),
    })
}

fn query_is_case_sensitive(query: &str) -> bool {
    query.chars().any(|character| character.is_uppercase())
}

fn equal_under_case(candidate: &str, query: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        candidate == query
    } else {
        candidate.eq_ignore_ascii_case(query)
    }
}

fn prefix_under_case(candidate: &str, query: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        candidate.starts_with(query)
    } else {
        candidate.to_lowercase().starts_with(&query.to_lowercase())
    }
}

fn substring_under_case(candidate: &str, query: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        candidate.contains(query)
    } else {
        candidate.to_lowercase().contains(&query.to_lowercase())
    }
}

// ---- signature help internals ----

/// The signature label with each parameter's byte span. `...` renders at its
/// formal position (after the variadic's preceding named parameters), so
/// span indexes line up with `active_parameter`'s display translation.
fn render_signature(db: &dyn Db, function: &FunctionType<'_>) -> (String, Vec<TextRange>) {
    let mut renderer = TypeRenderer::default();
    let mut parts: Vec<String> = Vec::new();
    for ty in &function.positional {
        parts.push(renderer.render(db, *ty));
    }
    for field in &function.named {
        let name = if field.optional {
            format!("[{}]", field.name.text(db))
        } else {
            field.name.text(db).to_owned()
        };
        parts.push(format!("{name}: {}", renderer.render(db, field.ty)));
    }
    if let Some(rest) = &function.variadic {
        let at = function.positional.len() + rest.preceding_named.min(function.named.len());
        parts.insert(at, format!("...: {}", renderer.render(db, rest.element)));
    }

    let mut label = String::from("fn(");
    let mut parameters = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            label.push_str(", ");
        }
        let start = TextSize::from(label.len() as u32);
        label.push_str(part);
        parameters.push(TextRange::new(start, TextSize::from(label.len() as u32)));
    }
    label.push_str(") -> ");
    label.push_str(&renderer.render(db, function.ret));
    (label, parameters)
}

/// The rendered parameter the cursor's argument targets (legacy algorithm:
/// matching works in slot space — positionals, then named, the rest slot
/// last — and translates to the display order, which interleaves `...` at
/// its formal position).
fn active_parameter(
    db: &dyn Db,
    function: &FunctionType<'_>,
    rendered_count: usize,
    arguments: &[Argument],
    hir: &semantics::hir::Module,
    cursor: TextSize,
) -> Option<usize> {
    if rendered_count == 0 {
        return None;
    }

    let positional_count = function.positional.len();
    let matchable_count = positional_count + function.named.len();
    let preceding_named = function
        .variadic
        .as_ref()
        .map(|variadic| variadic.preceding_named.min(function.named.len()));
    let variadic_slot = function.variadic.is_some().then_some(matchable_count);
    let display_index = |slot: usize| match preceding_named {
        Some(preceding) if slot == matchable_count => positional_count + preceding,
        Some(preceding) if slot >= positional_count + preceding => slot + 1,
        _ => slot,
    };
    let slot_for_name = |name: &str| {
        function
            .named
            .iter()
            .position(|field| field.name.text(db) == name)
            .map(|index| positional_count + index)
    };
    let positionally_fillable = |slot: usize| {
        slot < positional_count
            || match preceding_named {
                Some(preceding) => slot - positional_count < preceding,
                None => true,
            }
    };
    let first_open_slot = |consumed: &[bool]| {
        consumed
            .iter()
            .enumerate()
            .find(|(slot, taken)| !**taken && positionally_fillable(*slot))
            .map(|(slot, _)| slot)
    };

    // Every argument ending before the cursor is complete; the cursor sits on
    // the next one (possibly not written yet, right after a comma).
    let cursor_index = arguments
        .iter()
        .filter(|argument| {
            argument
                .value
                .is_some_and(|value| hir.expression(value).range.end() < cursor)
        })
        .count();

    let mut consumed = vec![false; matchable_count];
    for argument in arguments.iter().take(cursor_index) {
        match &argument.name {
            // A name matching no parameter is a named-argument error (named
            // arguments are never routed into `...`); it consumes no slot.
            Some(name) => {
                if let Some(slot) = slot_for_name(name) {
                    consumed[slot] = true;
                }
            }
            None => {
                if let Some(slot) = first_open_slot(&consumed) {
                    consumed[slot] = true;
                }
            }
        }
    }

    // The cursor's own named argument targets the parameter it names; a name
    // matching no declared parameter is absorbed by `...`.
    let named_target = arguments
        .get(cursor_index)
        .and_then(|argument| argument.name.as_deref())
        .and_then(|name| slot_for_name(name).or(variadic_slot));

    Some(
        named_target
            .or_else(|| first_open_slot(&consumed))
            .or(variadic_slot)
            .map(display_index)
            .unwrap_or(rendered_count - 1)
            .min(rendered_count - 1),
    )
}

// ---- inlay hint internals ----

/// Variables are presentable only when the hinted type is a function AT THE
/// TOP — the label generalizes them into binder names. A variable anywhere
/// else (including inside a function nested in a union) would render an
/// unanchored type parameter, so those types show nothing.
fn scheme_is_hintable(db: &dyn Db, scheme: &TypeScheme<'_>) -> bool {
    is_hintable(
        db,
        scheme.body,
        matches!(scheme.body.kind(db), TyKind::Function(_)),
    )
}

/// Whether every leaf of the type is presentable in a hint.
/// `variables_allowed` is true only under a function type, whose variables
/// the label generalizes into binder names.
fn is_hintable(db: &dyn Db, ty: Ty<'_>, variables_allowed: bool) -> bool {
    match ty.kind(db) {
        TyKind::Unknown => false,
        TyKind::Var(_) | TyKind::Rigid(_) => variables_allowed,
        TyKind::Any | TyKind::Null | TyKind::Scalar(_) => true,
        TyKind::Vector(element)
        | TyKind::NamedVector(element)
        | TyKind::List(element)
        | TyKind::NamedList(element) => is_hintable(db, *element, variables_allowed),
        TyKind::Tuple(items) => items
            .iter()
            .all(|&item| is_hintable(db, item, variables_allowed)),
        TyKind::Record(fields) => fields
            .iter()
            .all(|field| is_hintable(db, field.ty, variables_allowed)),
        TyKind::Union(members) => members
            .iter()
            .all(|&member| is_hintable(db, member, variables_allowed)),
        TyKind::Named(_, arguments) => arguments
            .iter()
            .all(|&argument| is_hintable(db, argument, variables_allowed)),
        TyKind::Function(function) => {
            function
                .positional
                .iter()
                .all(|&ty| is_hintable(db, ty, variables_allowed))
                && function
                    .named
                    .iter()
                    .all(|field| is_hintable(db, field.ty, variables_allowed))
                && function
                    .variadic
                    .as_ref()
                    .is_none_or(|rest| is_hintable(db, rest.element, variables_allowed))
                && is_hintable(db, function.ret, variables_allowed)
        }
    }
}

// ---- the occurrence engine ----

/// What the cursor's name resolves to.
enum Target<'db> {
    /// A variable slot inside one item (a local, parameter, or loop
    /// variable).
    Slot { item: Item<'db>, binding: BindingId },
    /// A project-defined global name: a top-level definition read across
    /// items and files.
    Global(String),
    /// A `@type`/`@alias` name inside `#:` annotations.
    TypeName(String),
    /// An S4 class or generic named in a string literal.
    S4 { name: String, kind: S4Kind },
}

fn target_at<'db>(
    db: &'db dyn Db,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
) -> Option<Target<'db>> {
    if let Some(cursor) = annotation_type_at(db, file, offset) {
        // Primitive type names have no project declaration to navigate to.
        let declared = cursor.navigable
            && type_name_occurrences(db, files, &cursor.name)
                .iter()
                .any(|occurrence| occurrence.is_declaration);
        return declared.then_some(Target::TypeName(cursor.name));
    }

    let position = position_in_item(db, file, offset)?;
    let naming = item_naming(db, position.item)?;

    if let Some(expression) = position.expression_at() {
        if let Some(binding) = naming.resolutions.get(&expression) {
            let info = naming.bindings.get(binding)?;
            // The item's own top-level binding IS the global: cross-item
            // reads resolve to the name, not the slot.
            if info.kind == BindingKind::TopLevel {
                return Some(Target::Global(info.name.clone()));
            }
            return Some(Target::Slot {
                item: position.item,
                binding: *binding,
            });
        }
        if let Some(name) = naming.non_locals.get(&expression) {
            // Only project-defined names have occurrences to offer; stub and
            // unresolved names resolve nowhere.
            let defined = global_declaration_exists(db, files, name);
            return defined.then(|| Target::Global(name.clone()));
        }
    }

    // S4 class/generic names live in string literals, invisible to naming.
    if let Some(occurrence) = s4_occurrences_in(db, file)
        .into_iter()
        .find(|occurrence| occurrence.range.start() <= offset && offset <= occurrence.range.end())
    {
        return Some(Target::S4 {
            name: occurrence.name,
            kind: occurrence.kind,
        });
    }

    // Parameter names and for-loop variables are declarations without an
    // expression; the cursor hits their binding site directly.
    for (id, info) in &naming.bindings {
        if matches!(info.kind, BindingKind::Parameter | BindingKind::ForVariable)
            && info.range.start() <= position.relative
            && position.relative <= info.range.end()
        {
            return Some(Target::Slot {
                item: position.item,
                binding: *id,
            });
        }
    }
    None
}

fn global_declaration_exists(db: &dyn Db, files: ProjectFiles, name: &str) -> bool {
    files.files(db).iter().any(|file| {
        item_tree(db, *file).into_iter().any(|item| {
            matches!(*item.kind(db), ItemKind::Function | ItemKind::Value)
                && item.name(db).as_deref() == Some(name)
        })
    })
}

fn occurrences(db: &dyn Db, files: ProjectFiles, target: &Target<'_>) -> Vec<Occurrence> {
    let mut result = Vec::new();
    match target {
        Target::Slot { item, binding } => {
            if let Some(node) = item_node(db, *item) {
                slot_occurrences(
                    db,
                    *item,
                    node.text_range().start(),
                    *binding,
                    &mut |range, is_declaration| {
                        result.push(Occurrence {
                            file: *item.file(db),
                            range,
                            is_declaration,
                        });
                    },
                );
            }
        }
        Target::TypeName(name) => {
            result = type_name_occurrences(db, files, name);
        }
        Target::S4 { name, kind } => {
            for &file in files.files(db) {
                for occurrence in s4_occurrences_in(db, file) {
                    if occurrence.name == *name && occurrence.kind == *kind {
                        result.push(Occurrence {
                            file,
                            range: occurrence.range,
                            is_declaration: occurrence.is_declaration,
                        });
                    }
                }
            }
        }
        Target::Global(name) => {
            for &file in files.files(db) {
                for item in item_tree(db, file) {
                    let Some(node) = item_node(db, item) else {
                        continue;
                    };
                    let item_offset = node.text_range().start();
                    // The defining item's own top-level slot carries the
                    // declaration target and any internal (recursive) reads.
                    if let Some(naming) = item_naming(db, item) {
                        for (id, info) in &naming.bindings {
                            if info.kind == BindingKind::TopLevel && info.name == *name {
                                slot_occurrences(
                                    db,
                                    item,
                                    item_offset,
                                    *id,
                                    &mut |range, is_declaration| {
                                        result.push(Occurrence {
                                            file,
                                            range,
                                            is_declaration,
                                        });
                                    },
                                );
                            }
                        }
                        // Reads that resolve outside the item: the global's
                        // uses from other definitions.
                        if let Some(hir) = item_hir(db, item) {
                            for (expression, non_local) in &naming.non_locals {
                                if non_local == name {
                                    result.push(Occurrence {
                                        file,
                                        range: hir.expression(*expression).range + item_offset,
                                        is_declaration: false,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let file_order = |file: SourceFile| {
        files
            .files(db)
            .iter()
            .position(|candidate| *candidate == file)
    };
    result.sort_by_key(|occurrence| (file_order(occurrence.file), occurrence.range.start()));
    result.dedup_by_key(|occurrence| (occurrence.file, occurrence.range));
    result
}

/// Feeds every occurrence of one slot inside its item to `emit`: the
/// binding's own site (parameters and loop variables are not expressions, so
/// resolutions alone would miss their declaration), then every resolved read
/// and assignment target.
fn slot_occurrences(
    db: &dyn Db,
    item: Item<'_>,
    item_offset: TextSize,
    binding: BindingId,
    emit: &mut dyn FnMut(TextRange, bool),
) {
    let Some(naming) = item_naming(db, item) else {
        return;
    };
    let Some(hir) = item_hir(db, item) else {
        return;
    };
    let Some(info) = naming.bindings.get(&binding) else {
        return;
    };

    let mut ranges: Vec<(TextRange, bool)> = Vec::new();
    if matches!(info.kind, BindingKind::Parameter | BindingKind::ForVariable) {
        ranges.push((info.range + item_offset, true));
    }

    let targets = assignment_targets(&hir);
    for (expression, resolved) in &naming.resolutions {
        if resolved == &binding {
            ranges.push((
                hir.expression(*expression).range + item_offset,
                targets.contains(expression),
            ));
        }
    }

    ranges.sort_by_key(|(range, _)| range.start());
    ranges.dedup_by_key(|(range, _)| *range);
    for (range, is_declaration) in ranges {
        emit(range, is_declaration);
    }
}

/// The set of expressions that are assignment targets — the write sites a
/// slot counts as declarations.
fn assignment_targets(hir: &semantics::hir::Module) -> std::collections::BTreeSet<ExprId> {
    let mut targets = std::collections::BTreeSet::new();
    for expression in &hir.expressions {
        if let ExpressionKind::Assign { target, .. } = &expression.kind {
            targets.insert(*target);
        }
    }
    targets
}

// ---- positions ----

/// The cursor's item and the item-relative cursor offset.
struct PositionedItem<'db> {
    db: &'db dyn Db,
    item: Item<'db>,
    item_offset: TextSize,
    relative: TextSize,
}

impl PositionedItem<'_> {
    /// The HIR expressions whose range contains the cursor, smallest first —
    /// end-inclusive, so a cursor sitting immediately after a name still hits
    /// it (the editor convention). Name references win ties.
    fn expressions_at(&self) -> Vec<ExprId> {
        let Some(hir) = item_hir(self.db, self.item) else {
            return Vec::new();
        };
        let mut containing: Vec<(TextSize, bool, ExprId)> = Vec::new();
        for (index, expression) in hir.expressions.iter().enumerate() {
            let range = expression.range;
            if range.is_empty() || !(range.start() <= self.relative && self.relative <= range.end())
            {
                continue;
            }
            containing.push((
                range.len(),
                !matches!(expression.kind, ExpressionKind::NameRef(_)),
                ExprId(index as u32),
            ));
        }
        containing.sort_by_key(|(width, not_name, id)| (*width, *not_name, id.0));
        containing.into_iter().map(|(_, _, id)| id).collect()
    }

    /// The smallest containing expression.
    fn expression_at(&self) -> Option<ExprId> {
        self.expressions_at().into_iter().next()
    }
}

fn position_in_item(db: &dyn Db, file: SourceFile, offset: TextSize) -> Option<PositionedItem<'_>> {
    // Strict containment wins; a cursor sitting exactly on an item's end
    // offset (typing at the end of the last statement) still belongs to it,
    // unless the next item starts there.
    let mut touching: Option<PositionedItem<'_>> = None;
    for item in item_tree(db, file) {
        let Some(node) = item_node(db, item) else {
            continue;
        };
        let range = node.text_range();
        if range.start() <= offset && offset < range.end() {
            return Some(PositionedItem {
                db,
                item,
                item_offset: range.start(),
                relative: offset - range.start(),
            });
        }
        if offset == range.end() && touching.is_none() {
            touching = Some(PositionedItem {
                db,
                item,
                item_offset: range.start(),
                relative: offset - range.start(),
            });
        }
    }
    touching
}

/// The name node range of a definition item (`name <- ...`), for goto
/// targets that land on the name rather than the whole statement.
fn definition_name_range(node: &syntax::SyntaxNode) -> Option<TextRange> {
    node.descendants()
        .find(|descendant| descendant.kind() == syntax::SyntaxKind::NAME)
        .map(|name| name.text_range())
}
