//! The fact-provider-generic IDE orchestration: the seven interactive features (hover, inlay hints,
//! signature help, definition, references, rename, completion) implemented over the [`IdeDatabase`]
//! (`super`) fact-provider trait, so the identical code serves both `analysis`'s retained state (the
//! frozen oracle) and the engine-backed view (`engine::ide_view`). Each function reads only facts the
//! database already holds and never drives a phase, so the caller refreshes the database first (the
//! `analysis::ide::*` wrappers run `lower`/`resolve_package`/`typecheck`; the engine primes its caches).
//!
//! Keep one `// Section` block per IDE action, a `// Symbol targets` block for the shared
//! identifier-resolution machinery, and one `// Utils` block for remaining shared helpers.
use {
    super::{
        COMPLETION_LIMIT, CodeAction, CodeActionKind, CompletionItem, CompletionItemKind,
        CompletionItemSource, CompletionResult, DebugSection, HoverInfo, IdeDatabase, InlayHint,
        Location, RenameResult, SignatureHelp, TextEdit,
    },
    crate::{
        diagnostic::{
            RenderedSignature, render_function_signature, render_generalized_type,
            render_user_facing_scheme,
        },
        document::{Document, DocumentId},
        hir::{
            Argument, AssignTarget, AssignmentScope, DefinitionId, DefinitionItem, DefinitionKind,
            Expression, ExpressionId, ExpressionKind, Module,
        },
        interner::Symbol,
        naming::{
            BindingId, BindingInfo, DocumentKind, NamesLocal, find_exported_binding,
            is_maybe_undefined_expression,
        },
        s4::{self, S4Constructor},
        text::{TextPosition, TextRange},
        type_syntax::{
            DocumentTypeToken, TypeTokenRole, render_named_type_ref, render_surface_type,
            type_tokens_in_range,
        },
        types::{Annotation, CoreType, FunctionType, RecordField, TypeAnnotationKind},
    },
    ropey::{Rope, iter::Chunks},
    std::{
        collections::{BTreeMap, BTreeSet},
        path::{Path, PathBuf},
    },
    tree_sitter::{Node, Point, Query, QueryCursor, Range, StreamingIterator, Tree},
};

//
// Hover
//

pub fn hover(database: &dyn IdeDatabase, path: &Path, position: TextPosition) -> Option<HoverInfo> {
    let document_id = database.document_id_for_path(path)?;
    let module = database.module(document_id)?;
    let document = database.document_by_id(document_id)?;
    let point = Point::new(position.line_index, position.character_index);

    // A cursor inside a `#:` type annotation resolves against the type notation, not the R expression
    // it decorates: the token under the cursor is re-lexed from the document and rendered on its own.
    if let Some(info) = type_annotation_hover(database, module, document, point) {
        return Some(info);
    }

    let target = hover_target_near(module, point)?;

    let mut contents = Vec::new();
    let mut debug = Vec::new();

    let range = match target {
        HoverTarget::Expression(expression_id, range) => {
            let expression = module.arena.try_get(expression_id)?;

            if let Some(core_type) = database.checked_expression_type(document_id, expression_id) {
                contents.push(code_block(&render_generalized_type(
                    database.interner(),
                    core_type,
                )));
            }
            if let Some(summary) = variable_definition_summary(database, document_id, expression_id)
            {
                contents.push(summary);
            }

            debug.push(DebugSection {
                title: "Lowering".to_owned(),
                body: code_block(&render_expression_hover(database, expression)),
            });
            if let Some(naming) =
                render_expression_naming_hover(database, document_id, expression_id)
            {
                debug.push(DebugSection {
                    title: "Naming".to_owned(),
                    body: code_block(&naming),
                });
            }
            debug.push(DebugSection {
                title: "Parsing".to_owned(),
                body: code_block(&render_range(range)),
            });
            range
        }
        HoverTarget::Definition(definition_id, range) => {
            let definition = module
                .definitions
                .iter()
                .find(|definition| definition.id == definition_id)?;
            contents.push(code_block(&render_definition_summary(database, definition)));
            debug.push(DebugSection {
                title: "Lowering".to_owned(),
                body: code_block(&render_definition_hover(database, definition)),
            });
            debug.push(DebugSection {
                title: "Parsing".to_owned(),
                body: code_block(&render_range(range)),
            });
            range
        }
    };

    // Without any primary content there is nothing useful to show, so report no hover.
    if contents.is_empty() {
        return None;
    }

    Some(HoverInfo {
        range: text_range(range),
        contents,
        debug,
    })
}

fn type_annotation_hover(
    database: &dyn IdeDatabase,
    module: &Module,
    document: &Document,
    point: Point,
) -> Option<HoverInfo> {
    let token = type_token_at(document, point)?;
    if token.role != TypeTokenRole::TypeName {
        return None;
    }

    // The name token of the `@type`/`@alias` declaration itself keeps the richer definition-target
    // hover (with the debug sections), so fall through when the cursor sits inside a definition the
    // token names.
    if module.definitions.iter().any(|definition| {
        range_contains_position(definition.range, point)
            && database.interner().resolve(definition.definition.name) == Some(token.text.as_str())
    }) {
        return None;
    }

    // The type name is resolved against the same document's `@type`/`@alias` declarations. Cross-file
    // resolution would need every package module primed on each hover keystroke, which the scoped hover
    // priming deliberately avoids; goto-definition (which primes more broadly) covers the cross-file case.
    let body = database
        .interner()
        .symbol_for(&token.text)
        .and_then(|symbol| type_definition_in(module, symbol))
        .map(|definition| render_definition_summary(database, definition))
        // A builtin scalar or otherwise undeclared type name has no `@type`/`@alias` definition to
        // expand (and may not even be interned), so show the name itself — hovering `integer` in
        // `list[integer]` still confirms what the cursor is on.
        .unwrap_or_else(|| token.text.clone());

    Some(HoverInfo {
        range: text_range(token.range),
        contents: vec![code_block(&body)],
        debug: Vec::new(),
    })
}

// The type-notation token under the cursor, if the cursor is inside a `#:` annotation block. The
// containing block is located textually (contiguous `#:` comment lines), then its notation is re-lexed
// to find the individual token the cursor points at. Locating the block from the text rather than from
// lowered annotation ranges covers every annotation position — expression-attached annotations,
// `@type`/`@alias` definition bodies (which attach to no expression), and blocks that fail to parse.
// Containment accepts the token's right edge, so a cursor sitting just after a name still hits it.
fn type_token_at(document: &Document, point: Point) -> Option<DocumentTypeToken> {
    let annotation_range = annotation_block_range_at(document.rope(), point)?;
    type_tokens_in_range(document.rope(), annotation_range)
        .into_iter()
        .find(|token| token_contains_position(token.range, point))
}

// The document range of the whole `#:` comment block containing the cursor: the cursor's own `#:`
// line extended over the contiguous `#:` lines above and below it (a multi-line annotation is one
// block; the type-notation re-lexer skips any line without the prefix, so over-approximation is safe).
fn annotation_block_range_at(rope: &Rope, point: Point) -> Option<Range> {
    if !is_annotation_line(rope, point.row) {
        return None;
    }

    let mut first_row = point.row;
    while first_row > 0 && is_annotation_line(rope, first_row - 1) {
        first_row -= 1;
    }
    let mut last_row = point.row;
    while is_annotation_line(rope, last_row + 1) {
        last_row += 1;
    }

    annotation_block_range(rope, first_row, last_row)
}

fn annotation_block_range(rope: &Rope, first_row: usize, last_row: usize) -> Option<Range> {
    let start_byte = rope.try_line_to_byte(first_row).ok()?;
    let last_line_start = rope.try_line_to_byte(last_row).ok()?;
    let last_line_length = rope.get_line(last_row)?.len_bytes();
    let end_byte = last_line_start + last_line_length;
    Some(Range {
        start_byte,
        end_byte,
        start_point: Point::new(first_row, 0),
        end_point: Point::new(last_row, last_line_length),
    })
}

fn is_annotation_line(rope: &Rope, row: usize) -> bool {
    rope.get_line(row)
        .is_some_and(|line| line.to_string().trim_start().starts_with("#:"))
}

// The `@type`/`@alias` declaration of `name` within a single module. `None` when the module declares no
// such type (a builtin scalar, an unknown name, or a type parameter).
fn type_definition_in(module: &Module, name: Symbol) -> Option<&DefinitionItem> {
    module
        .definitions
        .iter()
        .find(|definition| definition.definition.name == name)
}

//
// Inlay hints
//

// Inferred-type hints for unannotated `name <- value` bindings, shown after the binding name like
// rust-analyzer's let hints. Annotated bindings are skipped because the type is already written, and
// `Unknown` is skipped because it carries no information.
// `viewport` is an internal byte-offset range; only hints overlapping it are returned, so a client
// scrolled to one part of a large file does not pay for hints across the whole document. `None`
// returns hints for the entire file.
pub fn inlay_hints(
    database: &dyn IdeDatabase,
    path: &Path,
    viewport: Option<TextRange>,
) -> Vec<InlayHint> {
    let Some(document_id) = database.document_id_for_path(path) else {
        return Vec::new();
    };
    let Some(module) = database.module(document_id) else {
        return Vec::new();
    };

    let mut hints = Vec::new();
    for expression in module.arena.expressions() {
        // Only plain variable bindings are hinted; a replacement form (`x[i] <- v`) updates an
        // existing value whose binding is hinted at its own definition.
        let ExpressionKind::Assign {
            target: AssignTarget::Variable { range: target, .. },
            ..
        } = &expression.kind
        else {
            continue;
        };
        if let Some(viewport) = &viewport
            && !expression_overlaps_viewport(expression.range, viewport)
        {
            continue;
        }
        if expression.annotation.is_some() {
            continue;
        }
        let Some(core_type) = database.checked_expression_type(document_id, expression.id) else {
            continue;
        };
        if !is_hintable_type(core_type) {
            continue;
        }

        let label = format!(
            ": {}",
            render_generalized_type(database.interner(), core_type)
        );
        hints.push(InlayHint {
            position: TextPosition {
                line_index: target.end_point.row,
                character_index: target.end_point.column,
            },
            label,
        });
    }

    hints.sort_by_key(|hint| (hint.position.line_index, hint.position.character_index));
    hints
}

// Tree-sitter points and the internal `TextRange` share units (row, UTF-8 byte column), so the two
// ranges overlap iff neither lies entirely before the other. Touching endpoints count as overlap so
// a hint sitting on the viewport boundary is kept.
fn expression_overlaps_viewport(range: Range, viewport: &TextRange) -> bool {
    let expression_start = (range.start_point.row, range.start_point.column);
    let expression_end = (range.end_point.row, range.end_point.column);
    let viewport_start = (viewport.start.line_index, viewport.start.character_index);
    let viewport_end = (viewport.end.line_index, viewport.end.character_index);

    expression_start <= viewport_end && viewport_start <= expression_end
}

// Whether an inferred binding type makes a useful inline hint. A function type is hinted whenever it
// contains no `Unknown`: its free inference variables generalize into `<T>` binder names in the label,
// so `identity <- function(x) x` reads `<T> fn(x: T) -> T` exactly as hover shows it, keeping
// polymorphic and concrete function bindings consistently hinted. Any other type must be fully
// resolved — a loose type variable would render an unanchored type parameter as the whole hint, and
// `Unknown` carries no information — so partially-inferred values show nothing rather than noise.
fn is_hintable_type(core_type: &CoreType) -> bool {
    match core_type {
        CoreType::Function(_) => is_hint_renderable(core_type, true),
        _ => is_hint_renderable(core_type, false),
    }
}

// Whether every leaf of the type is presentable in a hint. `variables_allowed` is true only under a
// top-level function type, whose variables the hint label generalizes into binder names.
fn is_hint_renderable(core_type: &CoreType, variables_allowed: bool) -> bool {
    match core_type {
        CoreType::Unknown => false,
        CoreType::Variable(_) => variables_allowed,
        CoreType::List(inner_type) | CoreType::NamedList(inner_type) => {
            is_hint_renderable(inner_type, variables_allowed)
        }
        CoreType::Union(members) => members
            .iter()
            .all(|member| is_hint_renderable(member, variables_allowed)),
        CoreType::Nominal(_, type_arguments) => type_arguments
            .iter()
            .all(|type_argument| is_hint_renderable(type_argument, variables_allowed)),
        CoreType::Record(fields) => fields
            .iter()
            .all(|field| is_hint_renderable(&field.value, variables_allowed)),
        CoreType::Tuple(items) => items
            .iter()
            .all(|item| is_hint_renderable(item, variables_allowed)),
        CoreType::Function(function_type) => {
            function_type
                .parameters
                .iter()
                .all(|parameter| is_hint_renderable(parameter, variables_allowed))
                && function_type
                    .named_parameters
                    .iter()
                    .all(|parameter| is_hint_renderable(&parameter.value, variables_allowed))
                && function_type
                    .variadic
                    .as_ref()
                    .is_none_or(|variadic| is_hint_renderable(variadic, variables_allowed))
                && is_hint_renderable(&function_type.return_type, variables_allowed)
        }
        CoreType::Any
        | CoreType::Null
        | CoreType::Scalar(_)
        | CoreType::Vector(_)
        | CoreType::NamedVector(_) => true,
    }
}

//
// Signature help
//

// Shows the inferred signature of the function being called at the cursor, rendered as its
// generalized scheme (one renderer across the whole signature, so a polymorphic callee reads
// `<T, U> fn(x: list[T], f: fn(T) -> U) -> list[U]` rather than leaking raw inference variables).
// The active parameter follows R's argument matching: a named argument consumes the parameter it
// names, and a positional argument fills the first parameter not yet consumed. Needs checked types,
// so it is a no-op unless the callee resolved to a function type.
pub fn signature_help(
    database: &dyn IdeDatabase,
    path: &Path,
    position: TextPosition,
) -> Option<SignatureHelp> {
    let document_id = database.document_id_for_path(path)?;
    let module = database.module(document_id)?;
    let point = Point::new(position.line_index, position.character_index);

    let call_expression = module
        .arena
        .expressions()
        .iter()
        .filter(|expression| {
            matches!(expression.kind, ExpressionKind::Call { .. })
                && range_contains_position(expression.range, point)
        })
        .min_by_key(|expression| {
            (
                hover_target_width(expression.range),
                expression.range.start_byte,
                expression.id.0,
            )
        })?;

    let ExpressionKind::Call { callee, arguments } = &call_expression.kind else {
        return None;
    };

    let callee_type = database.checked_expression_type(document_id, *callee)?;
    let CoreType::Function(function_type) = callee_type else {
        return None;
    };

    let signature = render_function_signature(database.interner(), function_type);
    let active_parameter = active_parameter(function_type, &signature, arguments, module, point);

    Some(SignatureHelp {
        label: signature.label,
        parameters: signature.parameters,
        active_parameter,
    })
}

// The rendered parameter the cursor's argument targets, following the call-matching rules of the
// typing reference (which mirror R). The display slots mirror `RenderedSignature::parameters`:
// positional parameters, then named parameters, then the `...` slot when the function is variadic.
// A named argument targets the parameter it names; a positional argument fills the first open
// positionally-fillable slot — every parameter, except that an optional named parameter of a
// variadic function is matched by name only (it stands in for an R parameter declared after `...`,
// so surplus positional arguments flow to `...` instead). Arguments before the cursor consume their
// slots first, so `f(label = "x", <cursor>)` highlights the parameter `label` skipped over. With
// every slot taken and no `...`, the highlight stays on the last parameter rather than wrapping to
// a wrong one.
fn active_parameter(
    function_type: &FunctionType<CoreType>,
    signature: &RenderedSignature,
    arguments: &[Argument],
    module: &Module,
    point: Point,
) -> Option<usize> {
    if signature.parameters.is_empty() {
        return None;
    }

    let positional_count = function_type.parameters.len();
    let matchable_count = positional_count + function_type.named_parameters.len();
    let variadic_slot = function_type.variadic.is_some().then_some(matchable_count);
    let slot_for_name = |name: Symbol| {
        function_type
            .named_parameters
            .iter()
            .position(|parameter| parameter.name == name)
            .map(|index| positional_count + index)
    };
    let positionally_fillable = |slot: usize| {
        slot < positional_count
            || function_type
                .named_parameters
                .get(slot - positional_count)
                .is_some_and(|parameter| !(function_type.variadic.is_some() && parameter.optional))
    };
    let first_open_slot = |consumed: &[bool]| {
        consumed
            .iter()
            .enumerate()
            .find(|(slot, taken)| !**taken && positionally_fillable(*slot))
            .map(|(slot, _)| slot)
    };

    // The argument the cursor is at: every argument that ends before the cursor is complete, so the
    // cursor sits on the next one (which may not be written yet, e.g. right after a comma).
    let cursor_index = arguments
        .iter()
        .filter(|argument| {
            module
                .arena
                .try_get(argument.expression)
                .is_some_and(|expression| expression.range.end_point < point)
        })
        .count();

    let mut consumed = vec![false; matchable_count];
    for argument in arguments.iter().take(cursor_index) {
        match argument.name {
            // A name that matches no parameter is a named-parameter error per the typing reference
            // (named arguments are never routed into `...`); it consumes no slot either way.
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

    let named_target = arguments
        .get(cursor_index)
        .and_then(|argument| argument.name)
        .and_then(slot_for_name);

    Some(
        named_target
            .or_else(|| first_open_slot(&consumed))
            .or(variadic_slot)
            .unwrap_or(signature.parameters.len() - 1),
    )
}

fn code_block(body: &str) -> String {
    format!("```\n{body}\n```")
}

fn render_range(range: Range) -> String {
    format!(
        "range: {}:{} to {}:{}",
        range.start_point.row + 1,
        range.start_point.column + 1,
        range.end_point.row + 1,
        range.end_point.column + 1,
    )
}

// Where a hovered variable use is defined and whether the definition is file-local or a
// package-global, rendered as a human-readable line. Other expressions have no definition site.
fn variable_definition_summary(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    expression_id: ExpressionId,
) -> Option<String> {
    let local_naming = database.document_naming(document_id)?;

    if let Some(binding_id) = local_naming.expression_resolutions.get(&expression_id) {
        let binding = local_naming
            .bindings
            .get(binding_id)
            .expect("local hover binding should exist");
        let location = render_source_location(database, binding.module_id, binding.range);
        let mut summary = format!("Local variable, defined at `{location}`");
        if is_maybe_undefined_expression(local_naming, expression_id) {
            summary.push_str("\n\n_May be undefined on some paths._");
        }
        return Some(summary);
    }

    if let Some(symbol) = local_naming.non_locals.get(&expression_id)
        && let Some(package_naming) = database.package_naming()
        && let Some(export_document_id) = package_naming.global_bindings.get(symbol)
        && let Some(export_module) = database.module(*export_document_id)
        && let Some(export_document_naming) = database.document_naming(*export_document_id)
        && let Some(binding_id) =
            find_exported_binding(export_module, export_document_naming, *symbol)
        && let Some(binding) = export_document_naming.bindings.get(&binding_id)
    {
        let location = render_source_location(database, binding.module_id, binding.range);
        return Some(format!("Package global, defined at `{location}`"));
    }

    // A stdlib stub name (a non-local resolved to no package binding) reports its origin namespace so a
    // standard-library function reads as coming from its package (e.g. `lapply` from `base`).
    if let Some(symbol) = local_naming.non_locals.get(&expression_id)
        && let Some(namespace) = database.stub_namespace(*symbol)
    {
        return Some(format!("From the `{namespace}` package."));
    }

    None
}

fn render_definition_summary(database: &dyn IdeDatabase, definition: &DefinitionItem) -> String {
    let keyword = match definition.definition.kind {
        DefinitionKind::Type => "type",
        DefinitionKind::Alias => "alias",
    };
    let name = database
        .interner()
        .resolve(definition.definition.name)
        .unwrap_or("<unknown>");
    let type_parameters = if definition.definition.type_parameters.is_empty() {
        String::new()
    } else {
        let type_parameters = definition
            .definition
            .type_parameters
            .iter()
            .map(|symbol| {
                database
                    .interner()
                    .resolve(*symbol)
                    .unwrap_or("<unknown>")
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("<{type_parameters}>")
    };

    format!(
        "{keyword} {name}{type_parameters} = {}",
        render_surface_type(&definition.definition.surface_type, database.interner())
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoverTarget {
    Expression(ExpressionId, Range),
    Definition(DefinitionId, Range),
}

fn render_expression_hover(database: &dyn IdeDatabase, expression: &Expression) -> String {
    let mut lines = Vec::new();

    if let Some(annotation) = &expression.annotation {
        match annotation.annotation() {
            Annotation::Type { kind, surface_type } => {
                let prefix = match kind {
                    TypeAnnotationKind::Checked => "",
                    TypeAnnotationKind::UnknownOnly => "@if-unknown ",
                    TypeAnnotationKind::Trusted => "@trust ",
                };
                lines.push(format!(
                    "annotation: {prefix}{}",
                    render_surface_type(surface_type, database.interner())
                ));
            }
            Annotation::New { nominal_type } => {
                lines.push(format!(
                    "annotation: @new {}",
                    render_named_type_ref(nominal_type, database.interner())
                ));
            }
        }
    }

    lines.push(match &expression.kind {
        ExpressionKind::Null => "Null".to_owned(),
        ExpressionKind::Logical(value) => format!("Logical({value})"),
        ExpressionKind::Integer(value) => format!("Integer({value})"),
        ExpressionKind::Double(value) => format!("Double({value})"),
        ExpressionKind::Character(value) => format!("Character({value:?})"),
        ExpressionKind::AtomicConstant(atomic) => format!("AtomicConstant({atomic:?})"),
        ExpressionKind::StringLiteralName(symbol) => {
            let name = database.interner().resolve(*symbol).unwrap_or("<unknown>");
            format!("StringLiteralName({name:?})")
        }
        ExpressionKind::Symbol(symbol) => {
            let name = database.interner().resolve(*symbol).unwrap_or("<unknown>");
            format!("Symbol({name})")
        }
        ExpressionKind::Block {
            expressions,
            has_trailing_semicolon,
        } => format!(
            "Block(expressions: {}, trailing_semicolon: {has_trailing_semicolon})",
            expressions.len()
        ),
        ExpressionKind::Assign { target, scope, .. } => {
            let scope_suffix = match scope {
                AssignmentScope::Local => "",
                AssignmentScope::Enclosing => ", enclosing",
            };
            match target {
                AssignTarget::Variable { symbol, .. } => {
                    let name = database.interner().resolve(*symbol).unwrap_or("<unknown>");
                    format!("Assign({name}{scope_suffix})")
                }
                AssignTarget::Replacement { .. } => format!("Assign(<replacement>{scope_suffix})"),
            }
        }
        ExpressionKind::Function { parameters, .. } => {
            let parameters = parameters
                .iter()
                .map(|parameter| {
                    database
                        .interner()
                        .resolve(parameter.symbol)
                        .unwrap_or("<unknown>")
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("Function({parameters})")
        }
        ExpressionKind::Local { .. } => "Local".to_owned(),
        ExpressionKind::If { alternative, .. } => {
            format!("If(alternative: {})", alternative.is_some())
        }
        ExpressionKind::For { variable, .. } => {
            let name = database
                .interner()
                .resolve(*variable)
                .unwrap_or("<unknown>");
            format!("For({name})")
        }
        ExpressionKind::While { .. } => "While".to_owned(),
        ExpressionKind::Repeat { .. } => "Repeat".to_owned(),
        ExpressionKind::UnaryMinus { .. } => "UnaryMinus".to_owned(),
        ExpressionKind::UnaryNot { .. } => "UnaryNot".to_owned(),
        ExpressionKind::Call { arguments, .. } => {
            format!("Call(arguments: {})", arguments.len())
        }
        ExpressionKind::Subset { arguments, .. } => {
            format!("Subset(arguments: {})", arguments.len())
        }
        ExpressionKind::Subset2 { arguments, .. } => {
            format!("Subset2(arguments: {})", arguments.len())
        }
        ExpressionKind::Dollar { name, .. } => {
            let name = database.interner().resolve(*name).unwrap_or("<unknown>");
            format!("Dollar({name})")
        }
        ExpressionKind::Break => "Break".to_owned(),
        ExpressionKind::Next => "Next".to_owned(),
        ExpressionKind::Unsupported => "Unsupported".to_owned(),
    });

    lines.join("\n")
}

fn render_definition_hover(database: &dyn IdeDatabase, definition: &DefinitionItem) -> String {
    let name = database
        .interner()
        .resolve(definition.definition.name)
        .unwrap_or("<unknown>");
    let type_parameters = if definition.definition.type_parameters.is_empty() {
        String::new()
    } else {
        let type_parameters = definition
            .definition
            .type_parameters
            .iter()
            .map(|symbol| {
                database
                    .interner()
                    .resolve(*symbol)
                    .unwrap_or("<unknown>")
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("<{type_parameters}>")
    };

    let kind = match definition.definition.kind {
        DefinitionKind::Type => "TypeDefinition",
        DefinitionKind::Alias => "TypeAlias",
    };

    format!(
        "{kind} {name}{type_parameters} = {}",
        render_surface_type(&definition.definition.surface_type, database.interner())
    )
}

fn render_expression_naming_hover(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    expression_id: ExpressionId,
) -> Option<String> {
    let mut lines = Vec::new();
    let local_naming = database.document_naming(document_id)?;
    let mut non_local_symbol = None;

    if let Some(binding_id) = local_naming.expression_resolutions.get(&expression_id) {
        let binding = local_naming
            .bindings
            .get(binding_id)
            .expect("local hover binding should exist");
        lines.push(format!(
            "local resolution: {}",
            render_binding_site(database, binding)
        ));
        if is_maybe_undefined_expression(local_naming, expression_id) {
            lines.push("local warning: might be undefined".to_owned());
        }
    } else if let Some(symbol) = local_naming.non_locals.get(&expression_id) {
        let name = database.interner().resolve(*symbol).unwrap_or("<unknown>");
        lines.push(format!("local resolution: unresolved `{name}`"));
        non_local_symbol = Some(*symbol);
    }

    if let Some(symbol) = non_local_symbol
        && let Some(package_naming) = database.package_naming()
        && let Some(export_document_id) = package_naming.global_bindings.get(&symbol)
        && let Some(export_module) = database.module(*export_document_id)
        && let Some(export_document_naming) = database.document_naming(*export_document_id)
        && let Some(binding_id) =
            find_exported_binding(export_module, export_document_naming, symbol)
        && let Some(binding) = export_document_naming.bindings.get(&binding_id)
    {
        lines.push(format!(
            "package resolution: {}",
            render_binding_site(database, binding)
        ));
    }

    (!lines.is_empty()).then_some(lines.join("\n"))
}

fn render_binding_site(database: &dyn IdeDatabase, binding: &BindingInfo) -> String {
    let name = database
        .interner()
        .resolve(binding.symbol)
        .unwrap_or("<unknown>");
    format!(
        "binding `{name}` at {}",
        render_source_location(database, binding.module_id, binding.range)
    )
}

fn render_source_location(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    range: Range,
) -> String {
    let path = database
        .path_for_document_id(document_id)
        .map(|path| {
            path.strip_prefix(database.base_path())
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .unwrap_or_else(|| "<unknown>".to_owned());

    format!(
        "{path}:{}:{}",
        range.start_point.row + 1,
        range.start_point.column + 1
    )
}

//
// Definition
//

pub fn definition(
    database: &dyn IdeDatabase,
    path: &Path,
    position: TextPosition,
) -> Option<Vec<Location>> {
    let occurrences =
        symbol_occurrences_at(database, path, position, OccurrenceScope::Declaration)?;
    let mut definitions = occurrences
        .into_iter()
        .filter(|occurrence| occurrence.is_declaration)
        .map(|occurrence| occurrence.location)
        .collect::<Vec<_>>();
    // A local variable slot counts every write as a declaration; goto-definition targets the
    // slot's first write for a deterministic single answer (offering every reaching write is a
    // possible later refinement). Occurrences come in tree order, so the first is the first write.
    if cursor_is_local_slot(database, path, position) {
        definitions.truncate(1);
    }
    (!definitions.is_empty()).then_some(definitions)
}

fn cursor_is_local_slot(database: &dyn IdeDatabase, path: &Path, position: TextPosition) -> bool {
    let Some(document_id) = database.document_id_for_path(path) else {
        return false;
    };
    let Some(document) = database.document_by_id(document_id) else {
        return false;
    };
    identifier_at_position(document.tree(), position)
        .and_then(|identifier| symbol_target_for_identifier(database, document_id, identifier))
        .is_some_and(|target| matches!(target, SymbolTarget::Local { .. }))
}

//
// References
//

pub fn references(
    database: &dyn IdeDatabase,
    path: &Path,
    position: TextPosition,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    let occurrences = symbol_occurrences_at(database, path, position, OccurrenceScope::All)?;
    let references = occurrences
        .into_iter()
        .filter(|occurrence| include_declaration || !occurrence.is_declaration)
        .map(|occurrence| occurrence.location)
        .collect::<Vec<_>>();
    (!references.is_empty()).then_some(references)
}

//
// Rename
//

pub fn rename(
    database: &dyn IdeDatabase,
    path: &Path,
    position: TextPosition,
    new_name: &str,
) -> Option<RenameResult> {
    let occurrences = symbol_occurrences_at(database, path, position, OccurrenceScope::All)?;
    let mut edits = BTreeMap::<PathBuf, Vec<TextEdit>>::new();

    for occurrence in occurrences {
        edits
            .entry(occurrence.location.path)
            .or_default()
            .push(TextEdit {
                range: occurrence.location.range,
                replacement_text: new_name.to_owned(),
            });
    }

    (!edits.is_empty()).then_some(RenameResult { edits })
}

//
// Code actions
//

// The static quickfixes and source actions for a document range: removing or dot-prefixing an unused
// assignment (from the same reaching-write facts the `unused` diagnostic reports), and inserting an
// inferred `#:` type annotation above an unannotated binding (the text the inlay hint shows). All
// edits are computed eagerly — no resolve round-trip.
pub fn code_actions(database: &dyn IdeDatabase, path: &Path, range: TextRange) -> Vec<CodeAction> {
    let Some(document_id) = database.document_id_for_path(path) else {
        return Vec::new();
    };
    let Some(document) = database.document_by_id(document_id) else {
        return Vec::new();
    };
    let Some(module) = database.module(document_id) else {
        return Vec::new();
    };

    let mut actions = Vec::new();

    if let Some(naming) = database.document_naming(document_id) {
        for unused in &naming.unused_assignments {
            if !expression_overlaps_viewport(unused.range, &range) {
                continue;
            }
            let Some(name) = database.interner().resolve(unused.symbol) else {
                continue;
            };
            let Some((assignment_range, target_range)) =
                unused_assignment_ranges(module, unused.range)
            else {
                continue;
            };

            actions.push(CodeAction {
                title: format!("Remove unused assignment of `{name}`"),
                kind: CodeActionKind::RemoveUnusedAssignment,
                edits: single_file_edit(
                    path,
                    removal_edit_range(document.rope(), assignment_range),
                    String::new(),
                ),
            });
            actions.push(CodeAction {
                title: format!("Prefix `{name}` with `.` to keep it"),
                kind: CodeActionKind::PrefixDot,
                edits: single_file_edit(
                    path,
                    collapsed_range(target_range.start_point),
                    ".".to_owned(),
                ),
            });
        }
    }

    // Annotation inserts: the cursor's binding as a quickfix, plus one whole-file source action
    // covering every eligible binding.
    let mut all_annotation_edits = Vec::new();
    for expression in module.arena.expressions() {
        let Some(edit) = annotation_insert_edit(database, document_id, document, expression) else {
            continue;
        };
        all_annotation_edits.push(edit.clone());
        if expression_overlaps_viewport(expression.range, &range)
            && let ExpressionKind::Assign {
                target: AssignTarget::Variable { symbol, .. },
                ..
            } = &expression.kind
        {
            let name = database.interner().resolve(*symbol).unwrap_or("<unknown>");
            actions.push(CodeAction {
                title: format!("Add inferred type annotation for `{name}`"),
                kind: CodeActionKind::InsertInferredAnnotation,
                edits: BTreeMap::from([(path.to_path_buf(), vec![edit])]),
            });
        }
    }
    if !all_annotation_edits.is_empty() {
        actions.push(CodeAction {
            title: "Add inferred type annotations for the whole file".to_owned(),
            kind: CodeActionKind::AddMissingAnnotations,
            edits: BTreeMap::from([(path.to_path_buf(), all_annotation_edits)]),
        });
    }

    actions
}

// The assignment expression an unused-write range points at, tolerating either the whole-assignment
// range or the target-name range as the recorded site. Returns the whole assignment's range and the
// written name's own range.
fn unused_assignment_ranges(module: &Module, unused_range: Range) -> Option<(Range, Range)> {
    module.arena.expressions().iter().find_map(|expression| {
        let ExpressionKind::Assign {
            target: AssignTarget::Variable { range, .. },
            ..
        } = &expression.kind
        else {
            return None;
        };
        (expression.range == unused_range || *range == unused_range)
            .then_some((expression.range, *range))
    })
}

// The range removing an assignment deletes: the whole line(s) — trailing newline included — when the
// assignment is the only content on them, otherwise exactly the assignment's own range.
fn removal_edit_range(rope: &Rope, range: Range) -> TextRange {
    let only_content_on_lines = line_prefix_is_blank(rope, range.start_point)
        && line_suffix_is_blank(rope, range.end_point);
    if !only_content_on_lines {
        return text_range(range);
    }

    let last_row = rope.len_lines().saturating_sub(1);
    if range.end_point.row < last_row {
        return TextRange {
            start: TextPosition {
                line_index: range.start_point.row,
                character_index: 0,
            },
            end: TextPosition {
                line_index: range.end_point.row + 1,
                character_index: 0,
            },
        };
    }
    TextRange {
        start: TextPosition {
            line_index: range.start_point.row,
            character_index: 0,
        },
        end: TextPosition {
            line_index: range.end_point.row,
            character_index: rope
                .get_line(range.end_point.row)
                .map(|line| line.len_bytes())
                .unwrap_or(range.end_point.column),
        },
    }
}

fn line_prefix_is_blank(rope: &Rope, point: Point) -> bool {
    rope.get_line(point.row).is_some_and(|line| {
        line.to_string()
            .get(..point.column)
            .is_some_and(|prefix| prefix.trim().is_empty())
    })
}

fn line_suffix_is_blank(rope: &Rope, point: Point) -> bool {
    rope.get_line(point.row).is_some_and(|line| {
        line.to_string()
            .get(point.column..)
            .is_some_and(|suffix| suffix.trim().is_empty())
    })
}

// The `#: <inferred type>` line inserted above an unannotated binding, indented like the binding's
// line. `None` when the binding is annotated, is not a plain variable binding, does not start its
// line (inserting a line above would detach it), or has no hintable checked type — the same
// hintability rule the inlay hints use, so the inserted text is exactly the hint's.
fn annotation_insert_edit(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    document: &Document,
    expression: &Expression,
) -> Option<TextEdit> {
    let ExpressionKind::Assign {
        target: AssignTarget::Variable { .. },
        ..
    } = &expression.kind
    else {
        return None;
    };
    if expression.annotation.is_some() {
        return None;
    }
    if !line_prefix_is_blank(document.rope(), expression.range.start_point) {
        return None;
    }
    let core_type = database.checked_expression_type(document_id, expression.id)?;
    if !is_hintable_type(core_type) {
        return None;
    }

    let line = document.rope().get_line(expression.range.start_point.row)?;
    let indentation = line
        .to_string()
        .get(..expression.range.start_point.column)?
        .to_owned();
    let rendered = render_generalized_type(database.interner(), core_type);
    Some(TextEdit {
        range: collapsed_range(Point::new(expression.range.start_point.row, 0)),
        replacement_text: format!("{indentation}#: {rendered}\n"),
    })
}

fn single_file_edit(
    path: &Path,
    range: TextRange,
    replacement_text: String,
) -> BTreeMap<PathBuf, Vec<TextEdit>> {
    BTreeMap::from([(
        path.to_path_buf(),
        vec![TextEdit {
            range,
            replacement_text,
        }],
    )])
}

fn collapsed_range(point: Point) -> TextRange {
    let position = TextPosition {
        line_index: point.row,
        character_index: point.column,
    };
    TextRange {
        start: position,
        end: position,
    }
}

//
// Type definition
//

// Goto-type-definition: the nominal (`@type`-declared) type of the expression under the cursor,
// resolved to its declaration. Only a directly nominal checked type navigates — a structural type
// has no declaration to go to.
pub fn type_definition(
    database: &dyn IdeDatabase,
    path: &Path,
    position: TextPosition,
) -> Option<Vec<Location>> {
    let document_id = database.document_id_for_path(path)?;
    let module = database.module(document_id)?;
    let point = Point::new(position.line_index, position.character_index);

    let HoverTarget::Expression(expression_id, _) = hover_target_near(module, point)? else {
        return None;
    };
    let core_type = database.checked_expression_type(document_id, expression_id)?;
    let CoreType::Nominal(name, _) = core_type else {
        return None;
    };
    let name = database.interner().resolve(*name)?.to_owned();

    let declarations = type_name_occurrences(database, &name, OccurrenceScope::Declaration)
        .into_iter()
        .map(|occurrence| occurrence.location)
        .collect::<Vec<_>>();
    (!declarations.is_empty()).then_some(declarations)
}

//
// Symbol targets
//
// Definition, references, and rename all resolve the identifier under the cursor to one
// `SymbolTarget` and then scan the target's scope for identifiers resolving to the same target.

#[derive(Debug, Clone, PartialEq, Eq)]
struct SymbolOccurrence {
    location: Location,
    is_declaration: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolTarget {
    Local {
        document_id: DocumentId,
        binding_id: BindingId,
    },
    Global {
        symbol: Symbol,
        export_document_id: DocumentId,
    },
}

// `Declaration` scope restricts a global lookup to the single document that exports the symbol, so
// goto-definition does not scan the whole project. `All` scope is needed by references and rename,
// which must find every use across the package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OccurrenceScope {
    Declaration,
    All,
}

fn symbol_occurrences_at(
    database: &dyn IdeDatabase,
    path: &Path,
    position: TextPosition,
    scope: OccurrenceScope,
) -> Option<Vec<SymbolOccurrence>> {
    let document_id = database.document_id_for_path(path)?;
    let document = database.document_by_id(document_id)?;
    let point = Point::new(position.line_index, position.character_index);

    // A type name inside a `#:` annotation is invisible to the naming analysis, so it resolves
    // structurally against the project's `@type`/`@alias` declarations and every annotation's re-lexed
    // notation — one occurrence path shared by goto-definition, references, and rename.
    if let Some(token) = type_token_at(document, point)
        && token.role == TypeTokenRole::TypeName
    {
        return Some(type_name_occurrences(database, &token.text, scope));
    }

    // Identifiers resolve through the naming analysis (the common case).
    if let Some(identifier) = identifier_at_position(document.tree(), position)
        && let Some(target) = symbol_target_for_identifier(database, document_id, identifier)
    {
        return Some(identifier_occurrences(database, target, scope));
    }

    // S4 class/generic/method names are string literals, invisible to the naming analysis, so they
    // are resolved structurally instead. This keeps goto-definition, references, and rename on a
    // single occurrence-scanning path.
    if let Some(target) = s4_symbol_at(document.tree(), document.rope(), point) {
        return Some(s4_occurrences(database, &target, scope));
    }

    None
}

fn identifier_occurrences(
    database: &dyn IdeDatabase,
    target: SymbolTarget,
    scope: OccurrenceScope,
) -> Vec<SymbolOccurrence> {
    let document_ids = match target {
        SymbolTarget::Local { document_id, .. } => vec![document_id],
        SymbolTarget::Global {
            export_document_id, ..
        } => match scope {
            OccurrenceScope::Declaration => vec![export_document_id],
            OccurrenceScope::All => database.all_document_ids(),
        },
    };
    let mut occurrences = Vec::new();

    // Cheap text prefilter: an identifier whose spelled source differs from the target's name can
    // never resolve to the target, so skip full resolution for it. Same-spelled identifiers still
    // go through `symbol_target_for_identifier`, preserving shadowing correctness. If the name
    // cannot be resolved (it always should), fall back to resolving every identifier.
    let target_name = match target {
        SymbolTarget::Global { symbol, .. } => database.interner().resolve(symbol),
        SymbolTarget::Local {
            document_id,
            binding_id,
        } => database
            .document_naming(document_id)
            .and_then(|naming| naming.bindings.get(&binding_id))
            .and_then(|binding| database.interner().resolve(binding.symbol)),
    };

    for scoped_document_id in document_ids {
        let scoped_path = database
            .path_for_document_id(scoped_document_id)
            .unwrap_or_else(|| panic!("missing path for document {scoped_document_id:?}"))
            .to_path_buf();
        let scoped_document = database
            .document_by_id(scoped_document_id)
            .unwrap_or_else(|| panic!("missing document {scoped_document_id:?}"));

        for identifier in identifier_nodes(scoped_document.tree()) {
            if let Some(name) = target_name {
                let range = identifier.range();
                if range.end_byte - range.start_byte != name.len()
                    || scoped_document
                        .rope()
                        .byte_slice(range.start_byte..range.end_byte)
                        != name
                {
                    continue;
                }
            }

            if symbol_target_for_identifier(database, scoped_document_id, identifier)
                != Some(target)
            {
                continue;
            }

            occurrences.push(SymbolOccurrence {
                location: Location {
                    path: scoped_path.clone(),
                    range: text_range(identifier.range()),
                },
                is_declaration: identifier.parent().is_some_and(|parent| {
                    is_parameter_name(identifier, parent)
                        || is_assignment_target(identifier, parent)
                        || is_for_variable(identifier, parent)
                }),
            });
        }
    }

    occurrences
}

fn symbol_target_for_identifier(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    identifier: Node<'_>,
) -> Option<SymbolTarget> {
    if identifier.kind_id() != crate::tree::kind::IDENTIFIER
        || is_rhs_of_extract_or_namespace(identifier)
    {
        return None;
    }

    let module = database.module(document_id)?;
    let local_naming = database.document_naming(document_id)?;

    if let Some(parent) = identifier.parent() {
        if is_parameter_name(identifier, parent) {
            return local_naming.bindings.values().find_map(|binding| {
                (binding.range == identifier.range()).then_some(SymbolTarget::Local {
                    document_id,
                    binding_id: binding.id,
                })
            });
        }

        if is_assignment_target(identifier, parent)
            && let Some(expression_id) = module.expression_id_by_range(parent.range())
        {
            let binding_id = local_naming
                .expression_resolutions
                .get(&expression_id)
                .copied()?;
            return Some(symbol_target_for_binding(
                database,
                document_id,
                module,
                local_naming,
                binding_id,
            ));
        }

        if is_for_variable(identifier, parent)
            && let Some(binding_id) = local_naming
                .bindings
                .values()
                .find_map(|binding| (binding.range == parent.range()).then_some(binding.id))
        {
            return Some(symbol_target_for_binding(
                database,
                document_id,
                module,
                local_naming,
                binding_id,
            ));
        }
    }

    let expression_id = module.expression_id_by_range(identifier.range())?;
    if let Some(binding_id) = local_naming
        .expression_resolutions
        .get(&expression_id)
        .copied()
    {
        return Some(symbol_target_for_binding(
            database,
            document_id,
            module,
            local_naming,
            binding_id,
        ));
    }

    let symbol = local_naming.non_locals.get(&expression_id).copied()?;
    let package_naming = database.package_naming()?;
    let export_document_id = package_naming.global_bindings.get(&symbol).copied()?;
    Some(SymbolTarget::Global {
        symbol,
        export_document_id,
    })
}

fn symbol_target_for_binding(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    module: &Module,
    local_naming: &NamesLocal,
    binding_id: BindingId,
) -> SymbolTarget {
    let binding = local_naming
        .bindings
        .get(&binding_id)
        .unwrap_or_else(|| panic!("missing local binding {document_id:?}:{binding_id:?}"));

    if let Some(package_naming) = database.package_naming()
        && package_naming.global_bindings.get(&binding.symbol) == Some(&document_id)
        && let Some(exported_binding_id) =
            find_exported_binding(module, local_naming, binding.symbol)
        && exported_binding_id == binding_id
    {
        return SymbolTarget::Global {
            symbol: binding.symbol,
            export_document_id: document_id,
        };
    }

    SymbolTarget::Local {
        document_id,
        binding_id,
    }
}

//
// Annotation type names
//
// A `@type`/`@alias` name and its uses live inside `#:` comments, so the identifier machinery never
// sees them. Occurrences come from re-lexing every annotation block of every document: the declaration
// is the name token following a `@type`/`@alias` directive, and every other same-spelled type-name
// token is a reference. A block that binds the name as a type parameter (a `<T>` binder) shadows it,
// so that block's matching tokens are skipped — rename must never rewrite a shadowed use.

fn type_name_occurrences(
    database: &dyn IdeDatabase,
    name: &str,
    scope: OccurrenceScope,
) -> Vec<SymbolOccurrence> {
    let mut occurrences = Vec::new();
    for document_id in database.all_document_ids() {
        let path = database
            .path_for_document_id(document_id)
            .unwrap_or_else(|| panic!("missing path for document {document_id:?}"))
            .to_path_buf();
        let document = database
            .document_by_id(document_id)
            .unwrap_or_else(|| panic!("missing document {document_id:?}"));

        for block in annotation_blocks(document.rope()) {
            let tokens = type_tokens_in_range(document.rope(), block);
            if tokens
                .iter()
                .any(|token| token.role == TypeTokenRole::TypeParameter && token.text == name)
            {
                continue;
            }

            // The token right after a `@type`/`@alias` directive is the declared name.
            let mut declaration_pending = false;
            for token in tokens {
                match token.role {
                    TypeTokenRole::Directive => {
                        declaration_pending = matches!(token.text.as_str(), "@type" | "@alias");
                    }
                    TypeTokenRole::TypeName => {
                        let is_declaration = declaration_pending;
                        declaration_pending = false;
                        if token.text != name
                            || (scope == OccurrenceScope::Declaration && !is_declaration)
                        {
                            continue;
                        }
                        occurrences.push(SymbolOccurrence {
                            location: Location {
                                path: path.clone(),
                                range: text_range(token.range),
                            },
                            is_declaration,
                        });
                    }
                    _ => {}
                }
            }
        }
    }
    occurrences
}

// Every `#:` comment block of the document (maximal runs of contiguous `#:` lines), for the
// whole-document annotation scans (type-name references, rename).
fn annotation_blocks(rope: &Rope) -> Vec<Range> {
    let mut blocks = Vec::new();
    let mut row = 0;
    let total_lines = rope.len_lines();
    while row < total_lines {
        if !is_annotation_line(rope, row) {
            row += 1;
            continue;
        }
        let first_row = row;
        while row + 1 < total_lines && is_annotation_line(rope, row + 1) {
            row += 1;
        }
        if let Some(range) = annotation_block_range(rope, first_row, row) {
            blocks.push(range);
        }
        row += 1;
    }
    blocks
}

//
// S4 symbols
//
// S4 class, generic, and method names are written as string literals inside `setClass`/`setGeneric`/
// `setMethod`/`new` calls, so the identifier-based resolution above never sees them. They are
// resolved structurally here and fed through the same `SymbolOccurrence` machinery, so the S4 path
// shares goto-definition, references, and rename with ordinary symbols.

/// Whether the cursor sits on an S4 string-literal symbol (an S4 class/generic name). The engine's
/// definition priming uses this to decide it must scan every file's tree (the S4 path is project-wide)
/// rather than only the target plus its referenced exports.
pub fn cursor_is_s4_symbol(document: &Document, position: TextPosition) -> bool {
    let point = Point::new(position.line_index, position.character_index);
    s4_symbol_at(document.tree(), document.rope(), point).is_some()
}

// Whether the cursor sits on a type name inside a `#:` annotation. Goto-definition, references, and
// rename on such a name resolve cross-file over re-lexed annotation text, so the engine uses this to
// widen its prime to every document's parse.
pub fn cursor_on_annotation_type_name(document: &Document, position: TextPosition) -> bool {
    let point = Point::new(position.line_index, position.character_index);
    type_token_at(document, point).is_some_and(|token| token.role == TypeTokenRole::TypeName)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct S4Symbol {
    name: String,
    kind: S4SymbolKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum S4SymbolKind {
    // A class: declared by `setClass`, referenced by a `setMethod` signature and by `new`.
    Class,
    // A generic: declared by `setGeneric`, referenced by the function name of a `setMethod`.
    Generic,
}

struct S4Occurrence {
    symbol: S4Symbol,
    range: Range,
    is_declaration: bool,
}

fn s4_symbol_at(tree: &Tree, rope: &Rope, point: Point) -> Option<S4Symbol> {
    let mut occurrences = Vec::new();
    collect_s4_occurrences(tree.root_node(), rope, &mut occurrences);
    occurrences
        .into_iter()
        .find(|occurrence| range_contains_position(occurrence.range, point))
        .map(|occurrence| occurrence.symbol)
}

fn s4_occurrences(
    database: &dyn IdeDatabase,
    target: &S4Symbol,
    scope: OccurrenceScope,
) -> Vec<SymbolOccurrence> {
    let mut occurrences = Vec::new();
    for document_id in database.all_document_ids() {
        let document_path = database
            .path_for_document_id(document_id)
            .unwrap_or_else(|| panic!("missing path for document {document_id:?}"))
            .to_path_buf();
        let document = database
            .document_by_id(document_id)
            .unwrap_or_else(|| panic!("missing document {document_id:?}"));

        let mut document_occurrences = Vec::new();
        collect_s4_occurrences(
            document.tree().root_node(),
            document.rope(),
            &mut document_occurrences,
        );
        for occurrence in document_occurrences {
            if occurrence.symbol != *target {
                continue;
            }
            if scope == OccurrenceScope::Declaration && !occurrence.is_declaration {
                continue;
            }
            occurrences.push(SymbolOccurrence {
                location: Location {
                    path: document_path.clone(),
                    range: text_range(occurrence.range),
                },
                is_declaration: occurrence.is_declaration,
            });
        }
    }
    occurrences
}

fn collect_s4_occurrences(node: Node<'_>, rope: &Rope, out: &mut Vec<S4Occurrence>) {
    if node.kind_id() == crate::tree::kind::CALL {
        push_call_s4_occurrences(node, rope, out);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_s4_occurrences(child, rope, out);
    }
}

fn push_call_s4_occurrences(call: Node<'_>, rope: &Rope, out: &mut Vec<S4Occurrence>) {
    let Some(constructor) = s4::s4_constructor(call, rope) else {
        return;
    };
    let Some(arguments) = call.child_by_field_id(crate::tree::field::ARGUMENTS) else {
        return;
    };

    match constructor {
        S4Constructor::SetClass => push_string_occurrence(
            s4::string_argument(arguments, rope, "Class", 0),
            S4SymbolKind::Class,
            true,
            rope,
            out,
        ),
        S4Constructor::SetGeneric => push_string_occurrence(
            s4::string_argument(arguments, rope, "name", 0),
            S4SymbolKind::Generic,
            true,
            rope,
            out,
        ),
        S4Constructor::SetMethod => {
            push_string_occurrence(
                s4::string_argument(arguments, rope, "f", 0),
                S4SymbolKind::Generic,
                false,
                rope,
                out,
            );
            // The signature is a class name or a `c(...)` of class names.
            if let Some(signature) = s4::call_argument(arguments, rope, "signature", 1) {
                for class_string in s4::signature_class_strings(signature) {
                    push_string_occurrence(
                        Some(class_string),
                        S4SymbolKind::Class,
                        false,
                        rope,
                        out,
                    );
                }
            }
        }
        S4Constructor::New => push_string_occurrence(
            s4::string_argument(arguments, rope, "Class", 0),
            S4SymbolKind::Class,
            false,
            rope,
            out,
        ),
    }
}

fn push_string_occurrence(
    string_node: Option<Node<'_>>,
    kind: S4SymbolKind,
    is_declaration: bool,
    rope: &Rope,
    out: &mut Vec<S4Occurrence>,
) {
    let Some(string_node) = string_node else {
        return;
    };
    let Some(content) = s4::string_content(string_node) else {
        return;
    };
    let name = rope.byte_slice(content.byte_range()).to_string();
    if name.is_empty() {
        return;
    }
    out.push(S4Occurrence {
        symbol: S4Symbol { name, kind },
        range: content.range(),
        is_declaration,
    });
}

//
// Completion
//

// Matches the workspace-symbol cap. The full global namespace can exceed 20k entries; returning it
// all produces a huge payload and lets the client cache a complete list and stop re-querying.
pub fn completion(
    database: &dyn IdeDatabase,
    path: &Path,
    position: TextPosition,
) -> Option<CompletionResult> {
    let document_id = database.document_id_for_path(path)?;
    let document = database.document_by_id(document_id)?;
    let rope = document.rope();
    let tree = document.tree();
    let point = Point::new(position.line_index, position.character_index);

    // A cursor inside a `#:` annotation completes type names, not R values.
    if cursor_in_annotation_body(document, position) {
        return annotation_completion(database, rope, point);
    }

    // A cursor inside a string literal completes typed record fields when the string subscripts a
    // record (`x[["…"]]`) and is otherwise silent — R value names never resolve inside string
    // content, so the default namespace would be pure noise there.
    if string_node_at(tree, point).is_some() {
        return subset2_string_completion(database, document_id, point);
    }

    let (context, query) = extract_completion_context(position, rope)?;

    match context {
        CompletionContext::Default => {}
        CompletionContext::Field => {
            return Some(complete_result(rendered_query_matches(
                tree,
                rope,
                FIELD_QUERY,
                &query,
            )));
        }
        CompletionContext::Item => {
            return Some(dollar_completion(database, document_id, position, &query));
        }
        CompletionContext::Namespace => {
            return Some(complete_result(rendered_query_matches(
                tree,
                rope,
                NAMESPACE_QUERY,
                &query,
            )));
        }
        CompletionContext::MaybeNamespace => return None,
    }

    let mut items = Vec::new();

    // Keywords are a small fixed set, so they are prefix-completed rather than subsequence-matched;
    // no one searches for `function` by typing `con`.
    for keyword in RESERVED_WORDS {
        if query_prefix_matches(keyword, &query) {
            items.push(CompletionItem {
                label: (*keyword).to_owned(),
                kind: CompletionItemKind::Keyword,
                source: CompletionItemSource::Keyword,
                detail: None,
            });
        }
    }

    for local_item in local_completion_items(database, document_id, position, &query) {
        items.push(local_item);
    }

    if let Some(package_naming) = database.package_naming() {
        for (symbol, export_document_id) in &package_naming.global_bindings {
            let Some(label) = database.interner().resolve(*symbol) else {
                continue;
            };
            if !query_matches(label, &query) {
                continue;
            }

            let kind = global_completion_kind(database, *export_document_id, *symbol);
            items.push(CompletionItem {
                label: label.to_owned(),
                kind,
                source: CompletionItemSource::Global,
                detail: None,
            });
        }
    }

    // The standard-library corpus, with each stub's scheme as the item detail. A project global of
    // the same name outranks its stub at the deduplication step, mirroring how resolution shadows.
    for (symbol, scheme) in database.stub_schemes() {
        let Some(label) = database.interner().resolve(symbol) else {
            continue;
        };
        if !query_matches(label, &query) {
            continue;
        }
        items.push(CompletionItem {
            label: label.to_owned(),
            kind: match scheme.body {
                CoreType::Function(_) => CompletionItemKind::Function,
                _ => CompletionItemKind::Variable,
            },
            source: CompletionItemSource::Stdlib,
            detail: Some(render_user_facing_scheme(database.interner(), scheme)),
        });
    }

    deduplicate_completion_items(items, &query)
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

const FIELD_QUERY: &str = r#"(extract_operator operator: "@" rhs: (identifier) @ident)"#;
const ITEM_QUERY: &str = r#"(extract_operator operator: "$" rhs: (identifier) @ident)"#;
const NAMESPACE_QUERY: &str = r#"(namespace_operator rhs: (identifier) @ident)"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionContext {
    Default,
    Field,
    Item,
    Namespace,
    MaybeNamespace,
}

fn extract_completion_context(
    position: TextPosition,
    rope: &Rope,
) -> Option<(CompletionContext, String)> {
    let line = rope.get_line(position.line_index)?;
    // The column is a UTF-8 byte offset within the line, so scan only the bytes preceding the
    // cursor before walking their characters.
    let prefix = line.get_byte_slice(..position.character_index.min(line.len_bytes()))?;
    let mut previous = None;
    Some(prefix.chars().fold(
        (CompletionContext::Default, String::new()),
        |(mut context, mut query), character| {
            if character.is_alphabetic()
                || character == '.'
                || character == '_'
                || (!query.is_empty() && character.is_numeric())
            {
                query.push(character);
                // A name after a single `:` is the range operator's operand (`1:n`), not a pending
                // namespace access, so completion resumes; only a bare trailing `:` stays undecided.
                if context == CompletionContext::MaybeNamespace {
                    context = CompletionContext::Default;
                }
            } else {
                context = match character {
                    '@' => CompletionContext::Field,
                    '$' => CompletionContext::Item,
                    ':' => {
                        if previous.is_some_and(|previous_character| previous_character == ':') {
                            CompletionContext::Namespace
                        } else {
                            CompletionContext::MaybeNamespace
                        }
                    }
                    _ => CompletionContext::Default,
                };
                query.clear();
            }
            previous = Some(character);
            (context, query)
        },
    ))
}

//
// Typed field completion (`record$…` and `record[["…"]]`)
//

// `subject$…` completion: when the subject's checked type is a record, its fields complete with
// their rendered types; otherwise fall back to the textual scan over the document's other `$`
// accesses. A bare `subject$` (no field character yet) parses with no `$` expression to type, so
// the fallback marks itself incomplete — the client then re-queries on the next keystroke, when the
// partially-typed field name makes the subject typeable.
fn dollar_completion(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    position: TextPosition,
    query: &str,
) -> CompletionResult {
    if let Some(items) = typed_dollar_field_items(database, document_id, position, query) {
        return complete_result(items);
    }

    let Some(document) = database.document_by_id(document_id) else {
        return complete_result(Vec::new());
    };
    let mut result = complete_result(rendered_query_matches(
        document.tree(),
        document.rope(),
        ITEM_QUERY,
        query,
    ));
    result.is_incomplete = query.is_empty();
    result
}

fn typed_dollar_field_items(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    position: TextPosition,
    query: &str,
) -> Option<Vec<CompletionItem>> {
    let document = database.document_by_id(document_id)?;
    let module = database.module(document_id)?;

    // The `$` sits directly before the query characters; the innermost `$` expression covering it
    // owns the field position, and that expression's subject is what must be record-typed
    // (`a$b$…` completes the fields of `a$b`, not of `a`).
    let line_start = document.rope().try_line_to_byte(position.line_index).ok()?;
    let operator_byte = (line_start + position.character_index).checked_sub(query.len() + 1)?;
    let dollar = module
        .arena
        .expressions()
        .iter()
        .filter(|expression| {
            matches!(expression.kind, ExpressionKind::Dollar { .. })
                && expression.range.start_byte <= operator_byte
                && operator_byte < expression.range.end_byte
        })
        .min_by_key(|expression| expression.range.end_byte - expression.range.start_byte)?;
    let ExpressionKind::Dollar { value, .. } = &dollar.kind else {
        return None;
    };

    let subject_type = database.checked_expression_type(document_id, *value)?;
    let fields = record_fields(subject_type)?;
    Some(field_completion_items(database, fields, query))
}

// Completion inside the string subscript of `subject[["…"]]`: the subject's record fields complete
// as string contents. Inside any other string there is nothing to offer, so the answer is `None`.
fn subset2_string_completion(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    point: Point,
) -> Option<CompletionResult> {
    let document = database.document_by_id(document_id)?;
    let string_node = string_node_at(document.tree(), point)?;

    let argument = string_node.parent()?;
    if argument.kind_id() != crate::tree::kind::ARGUMENT {
        return None;
    }
    let arguments = argument.parent()?;
    if arguments.kind_id() != crate::tree::kind::ARGUMENTS {
        return None;
    }
    let subset2 = arguments.parent()?;
    if subset2.kind_id() != crate::tree::kind::SUBSET2 {
        return None;
    }
    let subject = subset2.child_by_field_id(crate::tree::field::FUNCTION)?;

    let module = database.module(document_id)?;
    let subject_id = module.expression_id_by_range(subject.range())?;
    let subject_type = database.checked_expression_type(document_id, subject_id)?;
    let fields = record_fields(subject_type)?;

    // The query is the string content already typed before the cursor.
    let content_start = string_node
        .child_by_field_id(crate::tree::field::OPEN)
        .map(|open| open.end_byte())?;
    let cursor_byte = document.rope().try_line_to_byte(point.row).ok()? + point.column;
    let query = if cursor_byte > content_start {
        document
            .rope()
            .byte_slice(content_start..cursor_byte)
            .to_string()
    } else {
        String::new()
    };

    Some(complete_result(field_completion_items(
        database, fields, &query,
    )))
}

// The string literal containing `point`, if any (walking up from the token under the cursor, so a
// position on the quotes or the content both count).
fn string_node_at(tree: &Tree, point: Point) -> Option<Node<'_>> {
    let node = tree.root_node().descendant_for_point_range(point, point)?;
    std::iter::successors(Some(node), |current| current.parent())
        .find(|current| current.kind_id() == crate::tree::kind::STRING)
}

fn record_fields(core_type: &CoreType) -> Option<&[RecordField<CoreType>]> {
    match core_type {
        CoreType::Record(fields) => Some(fields),
        _ => None,
    }
}

// Field items keep the record's declared order rather than re-ranking; a record rarely has enough
// fields for ranking to matter, and declaration order is the order the user wrote.
fn field_completion_items(
    database: &dyn IdeDatabase,
    fields: &[RecordField<CoreType>],
    query: &str,
) -> Vec<CompletionItem> {
    fields
        .iter()
        .filter_map(|field| {
            let label = database.interner().resolve(field.name)?.to_owned();
            if !query_matches(&label, query) {
                return None;
            }
            Some(CompletionItem {
                label,
                kind: CompletionItemKind::Field,
                source: CompletionItemSource::Field,
                detail: Some(render_generalized_type(database.interner(), &field.value)),
            })
        })
        .collect()
}

//
// Annotation type-name completion
//

// Whether the cursor sits inside the body of a `#:` annotation comment. The engine's completion
// priming uses the same predicate to decide it must prime every module (type-name completion lists
// the project's `@type`/`@alias` declarations, which may live in any file).
pub fn cursor_in_annotation_body(document: &Document, position: TextPosition) -> bool {
    annotation_query_at(
        document.rope(),
        Point::new(position.line_index, position.character_index),
    )
    .is_some()
}

// The partially-typed word before the cursor inside a `#:` annotation body, or `None` when the
// cursor is not in one — or is typing a directive (`@…`), which is not a type position.
fn annotation_query_at(rope: &Rope, point: Point) -> Option<String> {
    let line = rope.get_line(point.row)?.to_string();
    let trimmed_start = line.len() - line.trim_start().len();
    if !line[trimmed_start..].starts_with("#:") {
        return None;
    }
    let body_start = trimmed_start + "#:".len();
    if point.column < body_start {
        return None;
    }

    let prefix = line.get(..point.column)?;
    let query_start = prefix
        .char_indices()
        .rev()
        .take_while(|(_, character)| {
            character.is_alphanumeric() || *character == '_' || *character == '.'
        })
        .last()
        .map(|(index, _)| index)
        .unwrap_or(prefix.len());
    if prefix[..query_start].ends_with('@') {
        return None;
    }

    Some(prefix[query_start..].to_owned())
}

// The type names a `#:` annotation position can mention: the builtin names plus every project
// `@type`/`@alias` declaration.
const BUILTIN_TYPE_NAMES: &[&str] = &[
    "Any",
    "NULL",
    "Unknown",
    "character",
    "complex",
    "double",
    "fn",
    "integer",
    "list",
    "logical",
    "raw",
];

fn annotation_completion(
    database: &dyn IdeDatabase,
    rope: &Rope,
    point: Point,
) -> Option<CompletionResult> {
    let query = annotation_query_at(rope, point)?;
    let mut items = Vec::new();

    for name in BUILTIN_TYPE_NAMES {
        if query_matches(name, &query) {
            items.push(CompletionItem {
                label: (*name).to_owned(),
                kind: CompletionItemKind::Type,
                source: CompletionItemSource::Type,
                detail: None,
            });
        }
    }

    for document_id in database.all_document_ids() {
        let Some(module) = database.module(document_id) else {
            continue;
        };
        for definition in &module.definitions {
            let Some(label) = database.interner().resolve(definition.definition.name) else {
                continue;
            };
            if !query_matches(label, &query) {
                continue;
            }
            items.push(CompletionItem {
                label: label.to_owned(),
                kind: CompletionItemKind::Type,
                source: CompletionItemSource::Type,
                detail: Some(definition.definition.kind.directive_name().to_owned()),
            });
        }
    }

    deduplicate_completion_items(items, &query)
}

fn rendered_query_matches(
    tree: &Tree,
    rope: &Rope,
    query_text: &str,
    query: &str,
) -> Vec<CompletionItem> {
    let compiled_query =
        Query::new(&tree_sitter_r::LANGUAGE.into(), query_text).unwrap_or_else(|error| {
            panic!("failed to compile completion query `{query_text}`: {error}")
        });
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&compiled_query, tree.root_node(), RopeTextProvider(rope));
    let mut labels = Vec::new();

    while let Some(query_match) = matches.next() {
        let identifier = query_match.captures[0].node;
        let label = rope.byte_slice(identifier.byte_range()).to_string();
        if query_matches(&label, query) && label.len() != query.len() {
            labels.push(label);
        }
    }

    labels.sort();
    labels.dedup();

    labels
        .into_iter()
        .map(|label| CompletionItem {
            label,
            kind: CompletionItemKind::Variable,
            source: CompletionItemSource::Local,
            detail: None,
        })
        .collect()
}

fn local_completion_items(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    position: TextPosition,
    query: &str,
) -> Vec<CompletionItem> {
    let Some(document) = database.document_by_id(document_id) else {
        return Vec::new();
    };
    let point = Point::new(position.line_index, position.character_index);
    let Some(node) = node_at_position(document.tree(), point) else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for function_node in std::iter::successors(Some(node), |current| current.parent())
        .filter(|current| current.kind_id() == crate::tree::kind::FUNCTION_DEFINITION)
    {
        if let Some(parameters) = function_node.child_by_field_id(crate::tree::field::PARAMETERS) {
            for parameter in crate::tree::children_by_field(
                parameters,
                crate::tree::field::PARAMETER,
                &mut parameters.walk(),
            ) {
                let Some(name) = parameter.child_by_field_id(crate::tree::field::NAME) else {
                    continue;
                };
                if name.kind_id() != crate::tree::kind::IDENTIFIER {
                    continue;
                }

                let label = document.rope().byte_slice(name.byte_range()).to_string();
                if query_matches(&label, query) {
                    items.push(CompletionItem {
                        label,
                        kind: CompletionItemKind::Variable,
                        source: CompletionItemSource::Local,
                        detail: None,
                    });
                }
            }
        }

        if let Some(body) = function_node.child_by_field_id(crate::tree::field::BODY) {
            collect_local_bindings_in_body(document.rope(), body, query, &mut items);
        }
    }

    // A script's top-level bindings are document-local slots in scope at every position, so they
    // complete like locals. Package files skip this: their top-level bindings arrive through the
    // package-global namespace instead.
    if database.document_kind(document_id) == Some(DocumentKind::Script) {
        collect_local_bindings_in_body(
            document.rope(),
            document.tree().root_node(),
            query,
            &mut items,
        );
    }

    items
}

fn collect_local_bindings_in_body(
    rope: &Rope,
    node: Node<'_>,
    query: &str,
    items: &mut Vec<CompletionItem>,
) {
    for child in node.named_children(&mut node.walk()) {
        match child.kind_id() {
            crate::tree::kind::BINARY_OPERATOR => {
                let Some(lhs) = child.child_by_field_id(crate::tree::field::LHS) else {
                    continue;
                };
                let Some(operator) = child.child_by_field_id(crate::tree::field::OPERATOR) else {
                    continue;
                };
                if lhs.kind_id() != crate::tree::kind::IDENTIFIER
                    || ![crate::tree::kind::EQUAL, crate::tree::kind::LEFT_ASSIGN]
                        .contains(&operator.kind_id())
                {
                    continue;
                }

                let label = rope.byte_slice(lhs.byte_range()).to_string();
                if query_matches(&label, query) {
                    let kind = match child
                        .child_by_field_id(crate::tree::field::RHS)
                        .map(|rhs| rhs.kind_id())
                    {
                        Some(crate::tree::kind::FUNCTION_DEFINITION) => {
                            CompletionItemKind::Function
                        }
                        _ => CompletionItemKind::Variable,
                    };
                    items.push(CompletionItem {
                        label,
                        kind,
                        source: CompletionItemSource::Local,
                        detail: None,
                    });
                }
            }
            crate::tree::kind::FOR_STATEMENT => {
                let Some(variable) = child.child_by_field_id(crate::tree::field::VARIABLE) else {
                    continue;
                };
                if variable.kind_id() != crate::tree::kind::IDENTIFIER {
                    continue;
                }

                let label = rope.byte_slice(variable.byte_range()).to_string();
                if query_matches(&label, query) {
                    items.push(CompletionItem {
                        label,
                        kind: CompletionItemKind::Variable,
                        source: CompletionItemSource::Local,
                        detail: None,
                    });
                }
            }
            crate::tree::kind::FUNCTION_DEFINITION => continue,
            _ => {}
        }

        if child.child_count() > 0 {
            collect_local_bindings_in_body(rope, child, query, items);
        }
    }
}

fn global_completion_kind(
    database: &dyn IdeDatabase,
    document_id: DocumentId,
    symbol: Symbol,
) -> CompletionItemKind {
    let Some(module) = database.module(document_id) else {
        return CompletionItemKind::Variable;
    };
    let Some(local_naming) = database.document_naming(document_id) else {
        return CompletionItemKind::Variable;
    };
    let Some(binding_id) = find_exported_binding(module, local_naming, symbol) else {
        return CompletionItemKind::Variable;
    };

    binding_completion_kind(module, local_naming, binding_id)
}

fn binding_completion_kind(
    module: &Module,
    local_naming: &NamesLocal,
    binding_id: BindingId,
) -> CompletionItemKind {
    let Some(expression_id) = local_naming.expression_resolutions.iter().find_map(
        |(expression_id, resolved_binding_id)| {
            (*resolved_binding_id == binding_id).then_some(*expression_id)
        },
    ) else {
        return CompletionItemKind::Variable;
    };

    let Some(expression) = module.arena.try_get(expression_id) else {
        return CompletionItemKind::Variable;
    };
    match expression.kind {
        ExpressionKind::Assign { value, .. } => {
            match module
                .arena
                .try_get(value)
                .map(|expression| &expression.kind)
            {
                Some(ExpressionKind::Function { .. }) => CompletionItemKind::Function,
                _ => CompletionItemKind::Variable,
            }
        }
        _ => CompletionItemKind::Variable,
    }
}

fn complete_result(items: Vec<CompletionItem>) -> CompletionResult {
    CompletionResult {
        items,
        is_incomplete: false,
    }
}

fn deduplicate_completion_items(
    items: Vec<CompletionItem>,
    query: &str,
) -> Option<CompletionResult> {
    let mut seen = BTreeSet::new();
    let mut deduplicated = Vec::new();

    for item in items {
        if seen.insert(item.label.clone()) {
            deduplicated.push(item);
        }
    }

    // Rank by match quality first (prefix matches before scattered subsequence matches), then keep
    // the original source/label tiebreakers so equal-quality items stay stable and alphabetical.
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

    // Keep only the best-ranked window. Marking the list incomplete makes the client re-query as
    // the prefix narrows rather than filtering a truncated list locally.
    let is_incomplete = deduplicated.len() > COMPLETION_LIMIT;
    deduplicated.truncate(COMPLETION_LIMIT);

    (!deduplicated.is_empty()).then_some(CompletionResult {
        items: deduplicated,
        is_incomplete,
    })
}

//
// Utils
//

// The hover/type-definition target at the cursor. Ranges are end-exclusive, so a cursor at the very
// end of an expression (`foo|` at the end of a line) has no containing target; retry one column left
// and accept only a target that in fact ends at the cursor.
fn hover_target_near(module: &Module, point: Point) -> Option<HoverTarget> {
    if let Some(target) = hover_target_at(module, point) {
        return Some(target);
    }
    if point.column == 0 {
        return None;
    }
    let previous = Point::new(point.row, point.column - 1);
    hover_target_at(module, previous).filter(|target| {
        let range = match target {
            HoverTarget::Expression(_, range) | HoverTarget::Definition(_, range) => *range,
        };
        range.end_point == point
    })
}

fn hover_target_at(module: &Module, point: Point) -> Option<HoverTarget> {
    smallest_expression_hover_target(module, point)
        .or_else(|| smallest_definition_hover_target(module, point))
}

fn smallest_expression_hover_target(module: &Module, position: Point) -> Option<HoverTarget> {
    module
        .arena
        .expressions()
        .iter()
        .filter(|expression| range_contains_position(expression.range, position))
        .min_by_key(|expression| {
            (
                hover_target_width(expression.range),
                expression.range.start_byte,
                expression.id.0,
            )
        })
        .map(|expression| HoverTarget::Expression(expression.id, expression.range))
}

fn smallest_definition_hover_target(module: &Module, position: Point) -> Option<HoverTarget> {
    module
        .definitions
        .iter()
        .filter(|definition| range_contains_position(definition.range, position))
        .min_by_key(|definition| {
            (
                hover_target_width(definition.range),
                definition.range.start_byte,
                definition.id.0,
            )
        })
        .map(|definition| HoverTarget::Definition(definition.id, definition.range))
}

fn identifier_at_position<'tree>(tree: &'tree Tree, position: TextPosition) -> Option<Node<'tree>> {
    let point = Point::new(position.line_index, position.character_index);
    if let Some(node) = node_at_position(tree, point)
        && node.kind_id() == crate::tree::kind::IDENTIFIER
    {
        return Some(node);
    }

    // Ranges are end-exclusive, so a cursor sitting at an identifier's right edge (`foo|`) misses it;
    // retry one column left and accept only a node that in fact ends at the cursor.
    if position.character_index > 0 {
        let previous = Point::new(position.line_index, position.character_index - 1);
        if let Some(node) = node_at_position(tree, previous)
            && node.kind_id() == crate::tree::kind::IDENTIFIER
            && node.end_position() == point
        {
            return Some(node);
        }
    }

    None
}

fn node_at_position<'tree>(tree: &'tree Tree, point: Point) -> Option<Node<'tree>> {
    let node = tree.root_node().descendant_for_point_range(point, point)?;
    match node.kind_id() {
        crate::tree::kind::PROGRAM => match node.child(0) {
            Some(child) if point_in_range(point, child.range()) => {
                child.descendant_for_point_range(point, point)
            }
            _ => None,
        },
        _ => Some(node),
    }
}

fn point_in_range(point: Point, range: Range) -> bool {
    range.start_point <= point && point <= range.end_point
}

fn is_rhs_of_extract_or_namespace(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        [
            crate::tree::kind::EXTRACT_OPERATOR,
            crate::tree::kind::NAMESPACE_OPERATOR,
        ]
        .contains(&parent.kind_id())
            && parent
                .child_by_field_id(crate::tree::field::RHS)
                .is_some_and(|rhs| rhs.id() == node.id())
    })
}

fn is_assignment_target(identifier: Node<'_>, parent: Node<'_>) -> bool {
    if parent.kind_id() != crate::tree::kind::BINARY_OPERATOR {
        return false;
    }

    parent
        .child_by_field_id(crate::tree::field::LHS)
        .is_some_and(|lhs| lhs.id() == identifier.id())
        && parent
            .child_by_field_id(crate::tree::field::OPERATOR)
            .is_some_and(|operator| {
                [crate::tree::kind::EQUAL, crate::tree::kind::LEFT_ASSIGN]
                    .contains(&operator.kind_id())
            })
}

fn is_parameter_name(identifier: Node<'_>, parent: Node<'_>) -> bool {
    parent.kind_id() == crate::tree::kind::PARAMETER
        && parent
            .child_by_field_id(crate::tree::field::NAME)
            .is_some_and(|name| name.id() == identifier.id())
}

fn is_for_variable(identifier: Node<'_>, parent: Node<'_>) -> bool {
    parent.kind_id() == crate::tree::kind::FOR_STATEMENT
        && parent
            .child_by_field_id(crate::tree::field::VARIABLE)
            .is_some_and(|variable| variable.id() == identifier.id())
}

fn identifier_nodes(tree: &Tree) -> Vec<Node<'_>> {
    let mut cursor = tree.root_node().walk();
    let mut nodes = Vec::new();
    collect_identifier_nodes(&mut cursor, &mut nodes);
    nodes
}

fn collect_identifier_nodes<'tree>(
    cursor: &mut tree_sitter::TreeCursor<'tree>,
    nodes: &mut Vec<Node<'tree>>,
) {
    let node = cursor.node();
    if node.kind_id() == crate::tree::kind::IDENTIFIER {
        nodes.push(node);
    }

    if cursor.goto_first_child() {
        loop {
            collect_identifier_nodes(cursor, nodes);
            if !cursor.goto_next_sibling() {
                cursor.goto_parent();
                break;
            }
        }
    }
}

fn hover_target_width(range: Range) -> usize {
    range.end_byte - range.start_byte
}

fn range_contains_position(range: Range, position: Point) -> bool {
    !point_before(position, range.start_point) && point_before(position, range.end_point)
}

// Containment for token-under-cursor lookups: unlike the end-exclusive range rule, the token's right
// edge counts, so a cursor just after a type name (`Person|`) still hits it. Between two adjacent
// tokens the earlier one wins (lookups scan in source order).
fn token_contains_position(range: Range, position: Point) -> bool {
    !point_before(position, range.start_point) && !point_before(range.end_point, position)
}

fn point_before(left: Point, right: Point) -> bool {
    left.row < right.row || (left.row == right.row && left.column < right.column)
}

/// Ranking key for a search match; smaller compares as better. Ordered by match tier first (exact,
/// prefix, contiguous substring, scattered subsequence), then by the position of the first matched
/// character. Items with equal scores are left to the caller's own tiebreak (usually alphabetical).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchScore {
    tier: u8,
    first_match_index: u32,
}

const TIER_EXACT: u8 = 0;
const TIER_PREFIX: u8 = 1;
const TIER_SUBSTRING: u8 = 2;
const TIER_SUBSEQUENCE: u8 = 3;

/// Shortest query that is matched as a subsequence. Shorter queries fall back to prefix matching, so
/// a one- or two-character query in completion does not surface scattered, low-signal matches. This
/// mirrors rust-analyzer, which downgrades very short fuzzy inputs to prefix matching.
const MIN_SUBSEQUENCE_QUERY_LEN: usize = 3;

/// Match `query` against `candidate` the way rust-analyzer matches symbol-search and completion
/// queries, and return a [`MatchScore`] for ranking (or `None` when there is no match):
///
/// - The rule is **subsequence** matching: every character of `query` must appear in `candidate` in
///   order, not necessarily contiguously (so `Istrumnt` matches `instrument`).
/// - Matching is case-insensitive unless `query` contains an uppercase character, in which case it
///   becomes case-sensitive (**smart case**).
/// - Queries shorter than [`MIN_SUBSEQUENCE_QUERY_LEN`] use prefix matching instead of subsequence
///   matching, and an empty query matches everything.
///
/// The same function backs every search-like IDE feature (workspace symbols, completion) so they all
/// match and rank identically.
pub fn search_match(candidate: &str, query: &str) -> Option<MatchScore> {
    if query.is_empty() {
        return Some(MatchScore {
            tier: TIER_PREFIX,
            first_match_index: 0,
        });
    }

    let case_sensitive = query.chars().any(|character| character.is_uppercase());
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

    // A short query only matches as a prefix; a scattered subsequence of one or two characters is
    // almost always noise.
    if query.chars().count() < MIN_SUBSEQUENCE_QUERY_LEN && tier > TIER_PREFIX {
        return None;
    }

    Some(MatchScore {
        tier,
        first_match_index: first_match_index.unwrap_or(0),
    })
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

fn query_matches(label: &str, query: &str) -> bool {
    search_match(label, query).is_some()
}

fn query_prefix_matches(label: &str, query: &str) -> bool {
    query.is_empty()
        || prefix_under_case(
            label,
            query,
            query.chars().any(|character| character.is_uppercase()),
        )
}

fn text_range(range: Range) -> TextRange {
    TextRange {
        start: TextPosition {
            line_index: range.start_point.row,
            character_index: range.start_point.column,
        },
        end: TextPosition {
            line_index: range.end_point.row,
            character_index: range.end_point.column,
        },
    }
}

struct RopeTextProvider<'a>(&'a Rope);

struct RopeByteChunks<'a>(Chunks<'a>);

impl<'a> tree_sitter::TextProvider<&'a [u8]> for RopeTextProvider<'a> {
    type I = RopeByteChunks<'a>;

    fn text(&mut self, node: Node) -> Self::I {
        RopeByteChunks(self.0.byte_slice(node.byte_range()).chunks())
    }
}

impl<'a> Iterator for RopeByteChunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(str::as_bytes)
    }
}

#[cfg(test)]
mod search_match_tests {
    use super::search_match;

    fn matches(candidate: &str, query: &str) -> bool {
        search_match(candidate, query).is_some()
    }

    fn rank<'a>(query: &str, mut candidates: Vec<&'a str>) -> Vec<&'a str> {
        candidates.sort_by_key(|candidate| {
            (
                search_match(candidate, query).expect("candidate should match"),
                candidate.to_string(),
            )
        });
        candidates
    }

    #[test]
    fn subsequence_with_missing_characters_matches() {
        assert!(matches("instrument", "istrumnt"));
        assert!(matches("instrument", "inst"));
        assert!(matches("instrument", "itr")); // scattered, three characters
        assert!(matches("instrument", "instrument"));
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert!(!matches("instrument", "xyz"));
        assert!(!matches("instrument", "trx")); // `x` absent after `t`,`r`
        assert!(!matches("abc", "abcd")); // query longer than any subsequence
    }

    #[test]
    fn short_queries_only_prefix_match() {
        // one- and two-character queries fall back to prefix matching to avoid noise
        assert!(matches("instrument", "in"));
        assert!(!matches("instrument", "tr")); // scattered, but too short to fuzzy-match
        assert!(!matches("instrument", "ni")); // not a prefix
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(matches("anything", ""));
        assert!(matches("", ""));
    }

    #[test]
    fn matching_is_case_insensitive_for_lowercase_queries() {
        assert!(matches("Instrument", "istrumnt"));
        assert!(matches("INSTRUMENT", "inst"));
    }

    #[test]
    fn uppercase_query_forces_case_sensitivity() {
        // smart case: an uppercase query char only matches an uppercase candidate char
        assert!(matches("Instrument", "Ins"));
        assert!(!matches("instrument", "Ins"));
        assert!(matches("getHTTPResponse", "HTTP"));
    }

    #[test]
    fn exact_then_prefix_then_substring_then_subsequence_rank_in_order() {
        assert_eq!(
            rank(
                "inst",
                vec![
                    "my_instrument",
                    "reinstall",
                    "install",
                    "instrument",
                    "inst"
                ]
            ),
            vec![
                "inst",
                "install",
                "instrument",
                "reinstall",
                "my_instrument"
            ],
        );
    }

    #[test]
    fn contiguous_substring_outranks_scattered_subsequence() {
        // "ive" is contiguous in "derivative" (substring tier) but scattered in "is_verbose"
        let scored = |candidate: &str| search_match(candidate, "ive").unwrap();
        assert!(scored("derivative") < scored("is_verbose"));
    }

    #[test]
    fn score_is_comparable() {
        let exact = search_match("inst", "inst").unwrap();
        let prefix = search_match("instrument", "inst").unwrap();
        assert!(exact < prefix);
    }
}
