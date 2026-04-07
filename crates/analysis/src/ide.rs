// Keep one `// Section` block per IDE action, followed by one `// Utils` block for shared helpers.
use {
    crate::{
        analysis::{Analysis, lower, resolve_package},
        document::DocumentId,
        hir::{
            DefinitionId, DefinitionItem, DefinitionKind, Expression, ExpressionId, ExpressionKind,
            Module,
        },
        naming::BindingInfo,
        text::{TextPosition, TextRange},
        type_syntax::{render_named_type_ref, render_surface_type},
        types::{Annotation, TypeAnnotationKind},
    },
    std::path::Path,
};

//
// Hover
//

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    pub range: TextRange,
    pub sections: Vec<HoverSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverSection {
    pub phase: HoverPhase,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverPhase {
    Lowering,
    Naming,
    Typing,
}

pub fn hover(analysis: &mut Analysis, path: &Path, position: TextPosition) -> Option<HoverInfo> {
    let document_id = analysis.document_id_for_path(path)?;

    lower(analysis);
    resolve_package(analysis);

    let module = analysis.module(document_id)?;
    let point = tree_sitter::Point::new(position.line_index, position.character_index);
    let target = smallest_expression_hover_target(module, point)
        .or_else(|| smallest_definition_hover_target(module, point))?;
    let mut sections = Vec::new();

    match target {
        HoverTarget::Expression(expression_id, range) => {
            let expression = module.arena.get(expression_id);
            sections.push(HoverSection {
                phase: HoverPhase::Lowering,
                value: render_expression_hover(analysis, expression),
            });

            if let Some(value) =
                render_expression_naming_hover(analysis, document_id, expression_id)
            {
                sections.push(HoverSection {
                    phase: HoverPhase::Naming,
                    value,
                });
            }

            Some(HoverInfo {
                range: text_range(range),
                sections,
            })
        }
        HoverTarget::Definition(definition_id, range) => {
            let definition = module
                .definitions
                .iter()
                .find(|definition| definition.id == definition_id)?;
            sections.push(HoverSection {
                phase: HoverPhase::Lowering,
                value: render_definition_hover(analysis, definition),
            });

            Some(HoverInfo {
                range: text_range(range),
                sections,
            })
        }
    }
}

pub fn render_hover_markdown(hover_info: &HoverInfo, include_parsing: bool) -> String {
    let mut sections = hover_info
        .sections
        .iter()
        .map(|section| {
            let title = match section.phase {
                HoverPhase::Lowering => "Lowering",
                HoverPhase::Naming => "Naming",
                HoverPhase::Typing => "Typing",
            };
            format!("### {}\n\n```text\n{}\n```", title, section.value)
        })
        .collect::<Vec<_>>();

    if include_parsing {
        sections.push(format!(
            "### Parsing\n\n- range: {}:{} to {}:{}",
            hover_info.range.start.line_index + 1,
            hover_info.range.start.character_index + 1,
            hover_info.range.end.line_index + 1,
            hover_info.range.end.character_index + 1,
        ));
    }

    sections.join("\n\n---\n\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HoverTarget {
    Expression(ExpressionId, tree_sitter::Range),
    Definition(DefinitionId, tree_sitter::Range),
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
    } else if let Some(symbol) = local_naming.non_locals.get(&expression_id) {
        let name = analysis.interner().resolve(*symbol).unwrap_or("<unknown>");
        lines.push(format!("local resolution: unresolved `{name}`"));
        non_local_symbol = Some(*symbol);
    }

    if let Some(symbol) = non_local_symbol
        && let Some(package_naming) = analysis.package_naming()
        && let Some(export_document_id) = package_naming.global_bindings.get(&symbol)
        && let Some(export_document_naming) = analysis.document_naming(*export_document_id)
        && let Some(binding_id) = export_document_naming.global_exports.get(&symbol)
        && let Some(binding) = export_document_naming.bindings.get(binding_id)
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

fn render_source_location(
    analysis: &Analysis,
    document_id: DocumentId,
    range: tree_sitter::Range,
) -> String {
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
// Utils
//

fn smallest_expression_hover_target(
    module: &Module,
    position: tree_sitter::Point,
) -> Option<HoverTarget> {
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

fn smallest_definition_hover_target(
    module: &Module,
    position: tree_sitter::Point,
) -> Option<HoverTarget> {
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

fn hover_target_width(range: tree_sitter::Range) -> usize {
    range.end_byte - range.start_byte
}

fn range_contains_position(range: tree_sitter::Range, position: tree_sitter::Point) -> bool {
    !point_before(position, range.start_point) && point_before(position, range.end_point)
}

fn point_before(left: tree_sitter::Point, right: tree_sitter::Point) -> bool {
    left.row < right.row || (left.row == right.row && left.column < right.column)
}

fn text_range(range: tree_sitter::Range) -> TextRange {
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
