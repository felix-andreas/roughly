// Keep one `// Section` block per IDE action, a `// Symbol targets` block for the shared
// identifier-resolution machinery, and one `// Utils` block for remaining shared helpers.
use {
    crate::{
        analysis::{Analysis, lower, resolve_package, typecheck},
        diagnostic::render_core_type,
        document::DocumentId,
        hir::{
            DefinitionId, DefinitionItem, DefinitionKind, Expression, ExpressionId, ExpressionKind,
            Module,
        },
        interner::Symbol,
        naming::{BindingId, BindingInfo, find_exported_binding, is_maybe_undefined_expression},
        text::{TextPosition, TextRange},
        type_syntax::{render_named_type_ref, render_surface_type},
        types::{Annotation, CoreType, TypeAnnotationKind},
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
    let document_id = analysis.document_id_for_path(path)?;

    lower(analysis);
    resolve_package(analysis);
    // Typed hover needs checked expression types, which only `typecheck` retains. The typing phase
    // runs on demand for IDE features regardless of whether type-error diagnostics are surfaced; it
    // is incremental, so this is cheap when the package is already fresh.
    typecheck(analysis);

    let module = analysis.module(document_id)?;
    let point = Point::new(position.line_index, position.character_index);
    let target = smallest_expression_hover_target(module, point)
        .or_else(|| smallest_definition_hover_target(module, point))?;

    let mut contents = Vec::new();
    let mut debug = Vec::new();

    let range = match target {
        HoverTarget::Expression(expression_id, range) => {
            let expression = module.arena.try_get(expression_id)?;

            if let Some(core_type) = analysis.checked_expression_type(document_id, expression_id) {
                contents.push(code_block(&render_core_type(analysis.interner(), core_type)));
            }
            if let Some(summary) =
                variable_definition_summary(analysis, document_id, expression_id)
            {
                contents.push(summary);
            }

            debug.push(DebugSection {
                title: "Lowering".to_owned(),
                body: code_block(&render_expression_hover(analysis, expression)),
            });
            if let Some(naming) =
                render_expression_naming_hover(analysis, document_id, expression_id)
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
            contents.push(code_block(&render_definition_summary(analysis, definition)));
            debug.push(DebugSection {
                title: "Lowering".to_owned(),
                body: code_block(&render_definition_hover(analysis, definition)),
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

//
// Inlay hints
//

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub position: TextPosition,
    pub label: String,
}

// Inferred-type hints for unannotated `name <- value` bindings, shown after the binding name like
// rust-analyzer's let hints. Annotated bindings are skipped because the type is already written, and
// `Unknown` is skipped because it carries no information.
pub fn inlay_hints(analysis: &mut Analysis, path: &Path) -> Vec<InlayHint> {
    let Some(document_id) = analysis.document_id_for_path(path) else {
        return Vec::new();
    };

    lower(analysis);
    resolve_package(analysis);
    typecheck(analysis);

    let Some(module) = analysis.module(document_id) else {
        return Vec::new();
    };

    let mut hints = Vec::new();
    for expression in module.arena.expressions() {
        let ExpressionKind::Assign { target, .. } = &expression.kind else {
            continue;
        };
        if expression.annotation.is_some() {
            continue;
        }
        let Some(core_type) = analysis.checked_expression_type(document_id, expression.id) else {
            continue;
        };
        // Only concrete types make useful inline hints; a polymorphic or unknown binding would
        // render an internal type variable, which is noise shown on every line.
        if !is_concrete_type(core_type) {
            continue;
        }

        let name = analysis.interner().resolve(*target).unwrap_or_default();
        let label = format!(": {}", render_core_type(analysis.interner(), core_type));
        hints.push(InlayHint {
            position: TextPosition {
                line_index: expression.range.start_point.row,
                character_index: expression.range.start_point.column + name.len(),
            },
            label,
        });
    }

    hints.sort_by_key(|hint| (hint.position.line_index, hint.position.character_index));
    hints
}

fn is_concrete_type(core_type: &CoreType) -> bool {
    match core_type {
        CoreType::Variable(_) | CoreType::Unknown => false,
        CoreType::Nullable(inner_type)
        | CoreType::List(inner_type)
        | CoreType::NamedList(inner_type) => is_concrete_type(inner_type),
        CoreType::Nominal(_, type_arguments) => type_arguments.iter().all(is_concrete_type),
        CoreType::Record(fields) => fields.iter().all(|field| is_concrete_type(&field.value)),
        CoreType::Tuple(items) => items.iter().all(is_concrete_type),
        CoreType::Function(function_type) => {
            function_type.parameters.iter().all(is_concrete_type)
                && function_type
                    .named_parameters
                    .iter()
                    .all(|parameter| is_concrete_type(&parameter.value))
                && is_concrete_type(&function_type.return_type)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    pub label: String,
    pub parameters: Vec<String>,
    pub active_parameter: Option<usize>,
}

// Shows the inferred signature of the function being called at the cursor, with the active
// parameter derived from how many arguments precede the cursor. Needs checked types, so it is a
// no-op unless typing is enabled and the callee resolved to a function type.
pub fn signature_help(
    analysis: &mut Analysis,
    path: &Path,
    position: TextPosition,
) -> Option<SignatureHelp> {
    let document_id = analysis.document_id_for_path(path)?;

    lower(analysis);
    resolve_package(analysis);
    typecheck(analysis);

    let module = analysis.module(document_id)?;
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

    let callee_type = analysis.checked_expression_type(document_id, *callee)?;
    let CoreType::Function(function_type) = callee_type else {
        return None;
    };

    let mut parameters = Vec::new();
    for parameter in &function_type.parameters {
        parameters.push(render_core_type(analysis.interner(), parameter));
    }
    for parameter in &function_type.named_parameters {
        let name = analysis.interner().resolve(parameter.name).unwrap_or("");
        let rendered_name = if parameter.optional {
            format!("[{name}]")
        } else {
            name.to_owned()
        };
        parameters.push(format!(
            "{rendered_name}: {}",
            render_core_type(analysis.interner(), &parameter.value)
        ));
    }

    let label = format!(
        "fn({}) -> {}",
        parameters.join(", "),
        render_core_type(analysis.interner(), &function_type.return_type)
    );

    let active_parameter = if parameters.is_empty() {
        None
    } else {
        let preceding = arguments
            .iter()
            .filter(|argument| {
                module
                    .arena
                    .try_get(argument.expression)
                    .is_some_and(|expression| expression.range.end_point < point)
            })
            .count();
        Some(preceding.min(parameters.len() - 1))
    };

    Some(SignatureHelp {
        label,
        parameters,
        active_parameter,
    })
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
    analysis: &Analysis,
    document_id: DocumentId,
    expression_id: ExpressionId,
) -> Option<String> {
    let local_naming = analysis.document_naming(document_id)?;

    if let Some(binding_id) = local_naming.expression_resolutions.get(&expression_id) {
        let binding = local_naming
            .bindings
            .get(binding_id)
            .expect("local hover binding should exist");
        let location = render_source_location(analysis, binding.module_id, binding.range);
        let mut summary = format!("Local variable, defined at `{location}`");
        if is_maybe_undefined_expression(local_naming, expression_id) {
            summary.push_str("\n\n_May be undefined on some paths._");
        }
        return Some(summary);
    }

    if let Some(symbol) = local_naming.non_locals.get(&expression_id)
        && let Some(package_naming) = analysis.package_naming()
        && let Some(export_document_id) = package_naming.global_bindings.get(symbol)
        && let Some(export_module) = analysis.module(*export_document_id)
        && let Some(export_document_naming) = analysis.document_naming(*export_document_id)
        && let Some(binding_id) =
            find_exported_binding(export_module, export_document_naming, *symbol)
        && let Some(binding) = export_document_naming.bindings.get(&binding_id)
    {
        let location = render_source_location(analysis, binding.module_id, binding.range);
        return Some(format!("Package global, defined at `{location}`"));
    }

    None
}

fn render_definition_summary(analysis: &Analysis, definition: &DefinitionItem) -> String {
    let keyword = match definition.definition.kind {
        DefinitionKind::Type => "type",
        DefinitionKind::Alias => "alias",
    };
    let name = analysis
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
                analysis
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
        render_surface_type(&definition.definition.surface_type, analysis.interner())
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoverTarget {
    Expression(ExpressionId, Range),
    Definition(DefinitionId, Range),
}

fn render_expression_hover(analysis: &Analysis, expression: &Expression) -> String {
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
                    render_surface_type(surface_type, analysis.interner())
                ));
            }
            Annotation::New { nominal_type } => {
                lines.push(format!(
                    "annotation: @new {}",
                    render_named_type_ref(nominal_type, analysis.interner())
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
            let name = analysis.interner().resolve(*symbol).unwrap_or("<unknown>");
            format!("StringLiteralName({name:?})")
        }
        ExpressionKind::Symbol(symbol) => {
            let name = analysis.interner().resolve(*symbol).unwrap_or("<unknown>");
            format!("Symbol({name})")
        }
        ExpressionKind::Block {
            expressions,
            has_trailing_semicolon,
        } => format!(
            "Block(expressions: {}, trailing_semicolon: {has_trailing_semicolon})",
            expressions.len()
        ),
        ExpressionKind::Assign { target, .. } => {
            let name = analysis.interner().resolve(*target).unwrap_or("<unknown>");
            format!("Assign({name})")
        }
        ExpressionKind::Function { parameters, .. } => {
            let parameters = parameters
                .iter()
                .map(|parameter| {
                    analysis
                        .interner()
                        .resolve(parameter.symbol)
                        .unwrap_or("<unknown>")
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("Function({parameters})")
        }
        ExpressionKind::If { alternative, .. } => {
            format!("If(alternative: {})", alternative.is_some())
        }
        ExpressionKind::For { variable, .. } => {
            let name = analysis
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
            let name = analysis.interner().resolve(*name).unwrap_or("<unknown>");
            format!("Dollar({name})")
        }
        ExpressionKind::Unsupported => "Unsupported".to_owned(),
    });

    lines.join("\n")
}

fn render_definition_hover(analysis: &Analysis, definition: &DefinitionItem) -> String {
    let name = analysis
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
                analysis
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
        render_surface_type(&definition.definition.surface_type, analysis.interner())
    )
}

fn render_expression_naming_hover(
    analysis: &Analysis,
    document_id: DocumentId,
    expression_id: ExpressionId,
) -> Option<String> {
    let mut lines = Vec::new();
    let local_naming = analysis.document_naming(document_id)?;
    let mut non_local_symbol = None;

    if let Some(binding_id) = local_naming.expression_resolutions.get(&expression_id) {
        let binding = local_naming
            .bindings
            .get(binding_id)
            .expect("local hover binding should exist");
        lines.push(format!(
            "local resolution: {}",
            render_binding_site(analysis, binding)
        ));
        if is_maybe_undefined_expression(local_naming, expression_id) {
            lines.push("local warning: might be undefined".to_owned());
        }
    } else if let Some(symbol) = local_naming.non_locals.get(&expression_id) {
        let name = analysis.interner().resolve(*symbol).unwrap_or("<unknown>");
        lines.push(format!("local resolution: unresolved `{name}`"));
        non_local_symbol = Some(*symbol);
    }

    if let Some(symbol) = non_local_symbol
        && let Some(package_naming) = analysis.package_naming()
        && let Some(export_document_id) = package_naming.global_bindings.get(&symbol)
        && let Some(export_module) = analysis.module(*export_document_id)
        && let Some(export_document_naming) = analysis.document_naming(*export_document_id)
        && let Some(binding_id) =
            find_exported_binding(export_module, export_document_naming, symbol)
        && let Some(binding) = export_document_naming.bindings.get(&binding_id)
    {
        lines.push(format!(
            "package resolution: {}",
            render_binding_site(analysis, binding)
        ));
    }

    (!lines.is_empty()).then_some(lines.join("\n"))
}

fn render_binding_site(analysis: &Analysis, binding: &BindingInfo) -> String {
    let name = analysis
        .interner()
        .resolve(binding.symbol)
        .unwrap_or("<unknown>");
    format!(
        "binding `{name}` at {}",
        render_source_location(analysis, binding.module_id, binding.range)
    )
}

fn render_source_location(analysis: &Analysis, document_id: DocumentId, range: Range) -> String {
    let path = analysis
        .path_for_document_id(document_id)
        .map(|path| {
            path.strip_prefix(analysis.base_path())
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
    let occurrences = symbol_occurrences_at(analysis, path, position, OccurrenceScope::Declaration)?;
    let definitions = occurrences
        .into_iter()
        .filter(|occurrence| occurrence.is_declaration)
        .map(|occurrence| occurrence.location)
        .collect::<Vec<_>>();
    (!definitions.is_empty()).then_some(definitions)
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
    let occurrences = symbol_occurrences_at(analysis, path, position, OccurrenceScope::All)?;
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
    let occurrences = symbol_occurrences_at(analysis, path, position, OccurrenceScope::All)?;
    let mut edits = BTreeMap::<PathBuf, Vec<RenameEdit>>::new();

    for occurrence in occurrences {
        edits
            .entry(occurrence.location.path)
            .or_default()
            .push(RenameEdit {
                range: occurrence.location.range,
                replacement_text: new_name.to_owned(),
            });
    }

    (!edits.is_empty()).then_some(RenameResult { edits })
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
    analysis: &mut Analysis,
    path: &Path,
    position: TextPosition,
    scope: OccurrenceScope,
) -> Option<Vec<SymbolOccurrence>> {
    let document_id = analysis.document_id_for_path(path)?;

    lower(analysis);
    resolve_package(analysis);

    let document = analysis.document(path)?;

    // Identifiers resolve through the naming analysis (the common case).
    if let Some(identifier) = identifier_at_position(document.tree(), position)
        && let Some(target) = symbol_target_for_identifier(analysis, document_id, identifier)
    {
        return Some(identifier_occurrences(analysis, target, scope));
    }

    // S4 class/generic/method names are string literals, invisible to the naming analysis, so they
    // are resolved structurally instead. This keeps goto-definition, references, and rename on a
    // single occurrence-scanning path.
    let point = Point::new(position.line_index, position.character_index);
    if let Some(target) = s4_symbol_at(document.tree(), document.rope(), point) {
        return Some(s4_occurrences(analysis, &target, scope));
    }

    None
}

fn identifier_occurrences(
    analysis: &Analysis,
    target: SymbolTarget,
    scope: OccurrenceScope,
) -> Vec<SymbolOccurrence> {
    let document_ids = match target {
        SymbolTarget::Local { document_id, .. } => vec![document_id],
        SymbolTarget::Global {
            export_document_id, ..
        } => match scope {
            OccurrenceScope::Declaration => vec![export_document_id],
            OccurrenceScope::All => analysis.all_document_ids(),
        },
    };
    let mut occurrences = Vec::new();

    // Cheap text prefilter: an identifier whose spelled source differs from the target's name can
    // never resolve to the target, so skip full resolution for it. Same-spelled identifiers still
    // go through `symbol_target_for_identifier`, preserving shadowing correctness. If the name
    // cannot be resolved (it always should), fall back to resolving every identifier.
    let target_name = match target {
        SymbolTarget::Global { symbol, .. } => analysis.interner().resolve(symbol),
        SymbolTarget::Local {
            document_id,
            binding_id,
        } => analysis
            .document_naming(document_id)
            .and_then(|naming| naming.bindings.get(&binding_id))
            .and_then(|binding| analysis.interner().resolve(binding.symbol)),
    };

    for scoped_document_id in document_ids {
        let scoped_path = analysis
            .path_for_document_id(scoped_document_id)
            .unwrap_or_else(|| panic!("missing path for document {scoped_document_id:?}"))
            .to_path_buf();
        let scoped_document = analysis
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

            if symbol_target_for_identifier(analysis, scoped_document_id, identifier)
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
    analysis: &Analysis,
    document_id: DocumentId,
    identifier: Node<'_>,
) -> Option<SymbolTarget> {
    if identifier.kind_id() != crate::tree::kind::IDENTIFIER
        || is_rhs_of_extract_or_namespace(identifier)
    {
        return None;
    }

    let module = analysis.module(document_id)?;
    let local_naming = analysis.document_naming(document_id)?;

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
                analysis,
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
                analysis,
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
            analysis,
            document_id,
            module,
            local_naming,
            binding_id,
        ));
    }

    let symbol = local_naming.non_locals.get(&expression_id).copied()?;
    let package_naming = analysis.package_naming()?;
    let export_document_id = package_naming.global_bindings.get(&symbol).copied()?;
    Some(SymbolTarget::Global {
        symbol,
        export_document_id,
    })
}

fn symbol_target_for_binding(
    analysis: &Analysis,
    document_id: DocumentId,
    module: &Module,
    local_naming: &crate::naming::NamesLocal,
    binding_id: BindingId,
) -> SymbolTarget {
    let binding = local_naming
        .bindings
        .get(&binding_id)
        .unwrap_or_else(|| panic!("missing local binding {document_id:?}:{binding_id:?}"));

    if let Some(package_naming) = analysis.package_naming()
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
// S4 symbols
//
// S4 class, generic, and method names are written as string literals inside `setClass`/`setGeneric`/
// `setMethod`/`new` calls, so the identifier-based resolution above never sees them. They are
// resolved structurally here and fed through the same `SymbolOccurrence` machinery, so the S4 path
// shares goto-definition, references, and rename with ordinary symbols.

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
    analysis: &Analysis,
    target: &S4Symbol,
    scope: OccurrenceScope,
) -> Vec<SymbolOccurrence> {
    let mut occurrences = Vec::new();
    for document_id in analysis.all_document_ids() {
        let document_path = analysis
            .path_for_document_id(document_id)
            .unwrap_or_else(|| panic!("missing path for document {document_id:?}"))
            .to_path_buf();
        let document = analysis
            .document_by_id(document_id)
            .unwrap_or_else(|| panic!("missing document {document_id:?}"));

        let mut document_occurrences = Vec::new();
        collect_s4_occurrences(document.tree().root_node(), document.rope(), &mut document_occurrences);
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
    let Some(function_name) = call_function_name(call, rope) else {
        return;
    };
    let Some(arguments) = call.child_by_field_name("arguments") else {
        return;
    };

    match function_name.as_str() {
        "setClass" => push_string_occurrence(
            string_argument(arguments, rope, "Class", 0),
            S4SymbolKind::Class,
            true,
            rope,
            out,
        ),
        "setGeneric" => push_string_occurrence(
            string_argument(arguments, rope, "name", 0),
            S4SymbolKind::Generic,
            true,
            rope,
            out,
        ),
        "setMethod" => {
            push_string_occurrence(
                string_argument(arguments, rope, "f", 0),
                S4SymbolKind::Generic,
                false,
                rope,
                out,
            );
            // The signature is a class name or a `c(...)` of class names.
            if let Some(signature) = call_argument(arguments, rope, "signature", 1) {
                for class_string in signature_class_strings(signature) {
                    push_string_occurrence(Some(class_string), S4SymbolKind::Class, false, rope, out);
                }
            }
        }
        "new" => push_string_occurrence(
            string_argument(arguments, rope, "Class", 0),
            S4SymbolKind::Class,
            false,
            rope,
            out,
        ),
        _ => {}
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
    let Some(content) = string_node.child_by_field_name("content") else {
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

fn signature_class_strings<'tree>(signature: Node<'tree>) -> Vec<Node<'tree>> {
    if signature.kind_id() == crate::tree::kind::STRING {
        return vec![signature];
    }
    // `c("Person", "Other")`: collect the string elements.
    if signature.kind_id() == crate::tree::kind::CALL
        && let Some(arguments) = signature.child_by_field_name("arguments")
    {
        let mut cursor = arguments.walk();
        return arguments
            .children_by_field_name("argument", &mut cursor)
            .filter_map(|argument| argument.child_by_field_name("value"))
            .filter(|value| value.kind_id() == crate::tree::kind::STRING)
            .collect();
    }
    Vec::new()
}

fn call_function_name(call: Node<'_>, rope: &Rope) -> Option<String> {
    let function = call.child_by_field_name("function")?;
    match function.kind_id() {
        crate::tree::kind::IDENTIFIER => Some(rope.byte_slice(function.byte_range()).to_string()),
        crate::tree::kind::NAMESPACE_OPERATOR => {
            let rhs = function.child_by_field_name("rhs")?;
            (rhs.kind_id() == crate::tree::kind::IDENTIFIER)
                .then(|| rope.byte_slice(rhs.byte_range()).to_string())
        }
        _ => None,
    }
}

fn string_argument<'tree>(
    arguments: Node<'tree>,
    rope: &Rope,
    name: &str,
    index: usize,
) -> Option<Node<'tree>> {
    let argument = call_argument(arguments, rope, name, index)?;
    (argument.kind_id() == crate::tree::kind::STRING).then_some(argument)
}

// Resolves a call argument by name, falling back to the positional slot when it is unnamed.
fn call_argument<'tree>(
    arguments: Node<'tree>,
    rope: &Rope,
    name: &str,
    index: usize,
) -> Option<Node<'tree>> {
    let mut cursor = arguments.walk();
    for argument in arguments.children_by_field_name("argument", &mut cursor) {
        if let Some(argument_name) = argument.child_by_field_name("name")
            && rope.byte_slice(argument_name.byte_range()) == name
        {
            return argument.child_by_field_name("value");
        }
    }

    arguments
        .children_by_field_name("argument", &mut arguments.walk())
        .nth(index)
        .filter(|argument| argument.child_by_field_name("name").is_none())
        .and_then(|argument| argument.child_by_field_name("value"))
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

pub fn completion(
    analysis: &mut Analysis,
    path: &Path,
    position: TextPosition,
) -> Option<Vec<CompletionItem>> {
    let document_id = analysis.document_id_for_path(path)?;

    lower(analysis);
    resolve_package(analysis);

    let document = analysis.document(path)?;
    let rope = document.rope();
    let tree = document.tree();
    let (context, query) = extract_completion_context(position, rope)?;

    match context {
        CompletionContext::Default => {}
        CompletionContext::Field => {
            return Some(rendered_query_matches(tree, rope, FIELD_QUERY, &query));
        }
        CompletionContext::Item => {
            return Some(rendered_query_matches(tree, rope, ITEM_QUERY, &query));
        }
        CompletionContext::Namespace => {
            return Some(rendered_query_matches(tree, rope, NAMESPACE_QUERY, &query));
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
            });
        }
    }

    for local_item in local_completion_items(analysis, document_id, position, &query) {
        items.push(local_item);
    }

    if let Some(package_naming) = analysis.package_naming() {
        for (symbol, export_document_id) in &package_naming.global_bindings {
            let Some(label) = analysis.interner().resolve(*symbol) else {
                continue;
            };
            if !query_matches(label, &query) {
                continue;
            }

            let kind = global_completion_kind(analysis, *export_document_id, *symbol);
            items.push(CompletionItem {
                label: label.to_owned(),
                kind,
                source: CompletionItemSource::Global,
            });
        }
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
        })
        .collect()
}

fn local_completion_items(
    analysis: &Analysis,
    document_id: DocumentId,
    position: TextPosition,
    query: &str,
) -> Vec<CompletionItem> {
    let Some(document) = analysis.document_by_id(document_id) else {
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
            for parameter in parameters.children_by_field_name("parameter", &mut parameters.walk())
            {
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
                    });
                }
            }
        }

        if let Some(body) = function_node.child_by_field_id(crate::tree::field::BODY) {
            collect_local_bindings_in_body(document.rope(), body, query, &mut items);
        }
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
                    items.push(CompletionItem {
                        label,
                        kind: CompletionItemKind::Variable,
                        source: CompletionItemSource::Local,
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
    analysis: &Analysis,
    document_id: DocumentId,
    symbol: Symbol,
) -> CompletionItemKind {
    let Some(module) = analysis.module(document_id) else {
        return CompletionItemKind::Variable;
    };
    let Some(local_naming) = analysis.document_naming(document_id) else {
        return CompletionItemKind::Variable;
    };
    let Some(binding_id) = find_exported_binding(module, local_naming, symbol) else {
        return CompletionItemKind::Variable;
    };

    binding_completion_kind(module, local_naming, binding_id)
}

fn binding_completion_kind(
    module: &Module,
    local_naming: &crate::naming::NamesLocal,
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
            match module.arena.try_get(value).map(|expression| &expression.kind) {
                Some(ExpressionKind::Function { .. }) => CompletionItemKind::Function,
                _ => CompletionItemKind::Variable,
            }
        }
        _ => CompletionItemKind::Variable,
    }
}

fn deduplicate_completion_items(
    items: Vec<CompletionItem>,
    query: &str,
) -> Option<Vec<CompletionItem>> {
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

    (!deduplicated.is_empty()).then_some(deduplicated)
}

//
// Utils
//

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
    let node = node_at_position(tree, point)?;
    (node.kind_id() == crate::tree::kind::IDENTIFIER).then_some(node)
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
        || prefix_under_case(label, query, query.chars().any(|character| character.is_uppercase()))
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
            rank("inst", vec!["my_instrument", "reinstall", "install", "instrument", "inst"]),
            vec!["inst", "install", "instrument", "reinstall", "my_instrument"],
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
