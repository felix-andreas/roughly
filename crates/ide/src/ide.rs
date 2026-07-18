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
    let position = position_in_item(db, file, offset)?;
    let expression = position.expression_at()?;
    let check = item_check(db, position.item)?;
    let ty = *check.expression_types.get(&expression)?;

    let hir = item_hir(db, position.item)?;
    let mut renderer = TypeRenderer::default();
    let line = match &hir.expression(expression).kind {
        ExpressionKind::NameRef(name) => {
            format!("{name}: {}", renderer.render(db, ty))
        }
        _ => renderer.render(db, ty),
    };

    let range = hir.expression(expression).range + position.item_offset;
    Some(Hover {
        range,
        lines: vec![line],
    })
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
        let annotated = item_annotation_syntax(db, item).is_some();

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
                if !is_hintable(db, *ty, false) {
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

    let (callee, arguments) = hir
        .expressions
        .iter()
        .enumerate()
        .filter_map(|(index, expression)| match &expression.kind {
            ExpressionKind::Call { callee, arguments }
                if !expression.range.is_empty()
                    && expression.range.start() <= position.relative
                    && position.relative <= expression.range.end() =>
            {
                Some((expression.range, ExprId(index as u32), callee, arguments))
            }
            _ => None,
        })
        .min_by_key(|(range, id, ..)| (range.len(), range.start(), id.0))
        .map(|(_, _, callee, arguments)| (callee, arguments))?;

    let callee_ty = *check.expression_types.get(callee)?;
    let TyKind::Function(function) = callee_ty.kind(db) else {
        return None;
    };

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

fn scheme_is_hintable(db: &dyn Db, scheme: &TypeScheme<'_>) -> bool {
    is_hintable(db, scheme.body, false)
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
                .all(|&ty| is_hintable(db, ty, true))
                && function
                    .named
                    .iter()
                    .all(|field| is_hintable(db, field.ty, true))
                && function
                    .variadic
                    .as_ref()
                    .is_none_or(|rest| is_hintable(db, rest.element, true))
                && is_hintable(db, function.ret, true)
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
}

fn target_at<'db>(
    db: &'db dyn Db,
    files: ProjectFiles,
    file: SourceFile,
    offset: TextSize,
) -> Option<Target<'db>> {
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
    /// The smallest HIR expression whose range contains the cursor —
    /// end-inclusive, so a cursor sitting immediately after a name still hits
    /// it (the editor convention). Name references win ties.
    fn expression_at(&self) -> Option<ExprId> {
        let hir = item_hir(self.db, self.item)?;
        let mut best: Option<(TextSize, bool, ExprId)> = None;
        for (index, expression) in hir.expressions.iter().enumerate() {
            let range = expression.range;
            if range.is_empty() || !(range.start() <= self.relative && self.relative <= range.end())
            {
                continue;
            }
            let key = (
                range.len(),
                !matches!(expression.kind, ExpressionKind::NameRef(_)),
            );
            if best
                .as_ref()
                .is_none_or(|(width, not_name, _)| key < (*width, *not_name))
            {
                best = Some((key.0, key.1, ExprId(index as u32)));
            }
        }
        best.map(|(_, _, expression)| expression)
    }
}

fn position_in_item(db: &dyn Db, file: SourceFile, offset: TextSize) -> Option<PositionedItem<'_>> {
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
    }
    None
}

/// The name node range of a definition item (`name <- ...`), for goto
/// targets that land on the name rather than the whole statement.
fn definition_name_range(node: &syntax::SyntaxNode) -> Option<TextRange> {
    node.descendants()
        .find(|descendant| descendant.kind() == syntax::SyntaxKind::NAME)
        .map(|name| name.text_range())
}
