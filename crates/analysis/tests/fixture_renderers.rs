use analysis::{
    Analysis, Interner,
    document::DocumentId,
    hir::{
        DefinitionItem, DefinitionKind, ExpressionId, ExpressionKind, HirArena, Module, ModuleId,
    },
    lower::LoweringContext,
    naming::{BindingId, NamesGlobal, NamesLocal, find_binding, find_exported_binding},
    type_syntax::render_surface_type,
    typecheck::{InferenceError, InferenceState},
    types::CoreType,
};

pub fn render_expression_error_kind(error: &InferenceError) -> &'static str {
    match error {
        InferenceError::UnknownInferenceVariable(_) => "error: unknown inference variable",
        InferenceError::UnknownName { .. } => "error: unknown name",
        InferenceError::AliasCycle { .. } => "error: alias cycle",
        InferenceError::ExpectedFunction { .. } => "error: expected function",
        InferenceError::OccursCheckFailed { .. } => "error: occurs check failed",
        InferenceError::TypeMismatch { .. } => "error: type mismatch",
        InferenceError::UnresolvedAnnotationType { .. } => "error: unresolved annotation type",
        InferenceError::ConstraintViolation { .. } => "error: constraint violation",
        InferenceError::InvalidOperand { .. } => "error: invalid operand",
        InferenceError::TupleLengthMismatch { .. } => "error: tuple length mismatch",
        InferenceError::MixedListElements { .. } => "error: mixed list elements",
        InferenceError::RecordFieldMismatch { .. } => "error: record field mismatch",
        InferenceError::FunctionArityMismatch { .. } => "error: function arity mismatch",
        InferenceError::NamedParameterMismatch { .. } => "error: named parameter mismatch",
        InferenceError::NotAList { .. } => "error: not a list",
        InferenceError::FieldDoesNotExist { .. } => "error: field does not exist",
        InferenceError::PositionDoesNotExist { .. } => "error: position does not exist",
        InferenceError::NonLiteralSubscript { .. } => "error: non-literal subscript",
        InferenceError::UnsupportedSubset { .. } => "error: unsupported `[` subset",
        InferenceError::RecursionLimitExceeded => "error: recursion limit exceeded",
    }
}

pub fn render_expression_types(
    inference_state: &mut InferenceState,
    lowering_context: &LoweringContext,
    inferred_types: &[CoreType],
) -> String {
    let mut lines = Vec::with_capacity(inferred_types.len());

    for inferred_type in inferred_types {
        let resolved_type = inference_state
            .resolve(inferred_type.clone())
            .unwrap_or_else(|error| {
                panic!("inference result should resolve for rendering: {error:?}")
            });
        lines.push(analysis::render_core_type(
            lowering_context.interner(),
            &resolved_type,
        ));
    }

    lines.join("\n")
}

pub fn render_interface_snapshot(
    module: &analysis::Module,
    inference_state: &InferenceState,
    lowering_context: &LoweringContext,
) -> String {
    let mut exported_entries = Vec::<(usize, analysis::Symbol, String)>::new();

    for (index, definition_item) in module.definitions.iter().enumerate() {
        let definition = &definition_item.definition;
        let rendered_name = lowering_context
            .interner()
            .resolve(definition.name)
            .unwrap_or("<unknown>");
        let rendered_parameters = if definition.type_parameters.is_empty() {
            String::new()
        } else {
            format!(
                "<{}>",
                definition
                    .type_parameters
                    .iter()
                    .map(|parameter| {
                        lowering_context
                            .interner()
                            .resolve(*parameter)
                            .unwrap_or("<unknown>")
                            .to_owned()
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let label = match definition.kind {
            DefinitionKind::Type => "type",
            DefinitionKind::Alias => "alias",
        };
        exported_entries.push((
            index,
            definition.name,
            format!(
                "{label} {rendered_name}{rendered_parameters} = {}",
                render_surface_type(&definition.surface_type, lowering_context.interner())
            ),
        ));
    }

    let definition_count = module.definitions.len();
    for (expression_index, expression_id) in module.expressions.iter().enumerate() {
        let expression = module.arena.get(*expression_id);
        if let Some(target) = expression.kind.assignment_variable() {
            let name = lowering_context
                .interner()
                .resolve(target)
                .unwrap_or("<unknown>");
            let binding = inference_state
                .lookup_name(target)
                .unwrap_or_else(|| panic!("binding `{name}` should be present after inference"));
            exported_entries.push((
                definition_count + expression_index,
                target,
                format!(
                    "{name}: {}",
                    analysis::render_type_scheme(lowering_context.interner(), &binding.type_scheme)
                ),
            ));
        }
    }

    let mut final_entries = std::collections::BTreeMap::new();
    for (index, symbol, rendered_entry) in exported_entries {
        final_entries.insert(symbol, (index, rendered_entry));
    }

    let mut ordered_entries = final_entries.into_values().collect::<Vec<_>>();
    ordered_entries.sort_by_key(|(index, _)| *index);
    ordered_entries
        .into_iter()
        .map(|(_, rendered_entry)| rendered_entry)
        .collect::<Vec<_>>()
        .join("\n")
}

// Threads the full set of naming-phase artifacts needed to render resolved names; grouping them
// would add indirection to a test-only renderer without simplifying anything.
#[allow(clippy::too_many_arguments)]
pub fn render_named_hir(
    analysis: &Analysis,
    document_id: ModuleId,
    module: &Module,
    local_naming_result: &NamesLocal,
    all_local_naming: &std::collections::HashMap<DocumentId, NamesLocal>,
    global_naming_result: &NamesGlobal,
    binding_display_labels: &std::collections::BTreeMap<(DocumentId, BindingId), String>,
    interner: &Interner,
) -> String {
    let mut lines = Vec::new();

    for definition in &module.definitions {
        render_named_definition(definition, interner, 0, &mut lines);
    }

    for expression_id in &module.expressions {
        render_named_expression(
            analysis,
            document_id,
            &module.arena,
            *expression_id,
            local_naming_result,
            all_local_naming,
            global_naming_result,
            binding_display_labels,
            interner,
            0,
            &mut lines,
        );
    }

    lines.join("\n")
}

pub fn render_locally_named_hir(
    module_id: ModuleId,
    module: &Module,
    local_naming_result: &NamesLocal,
    interner: &Interner,
) -> String {
    let mut lines = Vec::new();

    for definition in &module.definitions {
        render_named_definition(definition, interner, 0, &mut lines);
    }

    for expression_id in &module.expressions {
        render_locally_named_expression(
            module_id,
            &module.arena,
            *expression_id,
            local_naming_result,
            interner,
            0,
            &mut lines,
        );
    }

    lines.join("\n")
}

fn render_named_definition(
    definition_item: &DefinitionItem,
    interner: &Interner,
    indent: usize,
    lines: &mut Vec<String>,
) {
    let prefix = "  ".repeat(indent);
    let definition = &definition_item.definition;
    let rendered_name = interner.resolve(definition.name).unwrap_or("<unknown>");
    let rendered_parameters = if definition.type_parameters.is_empty() {
        String::new()
    } else {
        format!(
            "<{}>",
            definition
                .type_parameters
                .iter()
                .map(|parameter| {
                    interner
                        .resolve(*parameter)
                        .unwrap_or("<unknown>")
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let label = match definition.kind {
        DefinitionKind::Type => "TypeDefinition",
        DefinitionKind::Alias => "TypeAlias",
    };
    lines.push(format!(
        "{prefix}{label}({rendered_name}{rendered_parameters} = {})",
        render_surface_type(&definition.surface_type, interner)
    ));
}

// Threads the full set of naming-phase artifacts needed to render resolved names; grouping them
// would add indirection to a test-only renderer without simplifying anything.
#[allow(clippy::too_many_arguments)]
fn render_named_expression(
    analysis: &Analysis,
    document_id: DocumentId,
    arena: &HirArena,
    expression_id: ExpressionId,
    local_naming_result: &NamesLocal,
    all_local_naming: &std::collections::HashMap<DocumentId, NamesLocal>,
    global_naming_result: &NamesGlobal,
    binding_display_labels: &std::collections::BTreeMap<(DocumentId, BindingId), String>,
    interner: &Interner,
    indent: usize,
    lines: &mut Vec<String>,
) {
    let expression = arena.get(expression_id);
    let prefix = "  ".repeat(indent);
    let render_binding_label = |resolved_document_id, binding_id| {
        binding_display_labels
            .get(&(resolved_document_id, binding_id))
            .cloned()
            .unwrap_or_else(|| format!("b{}", binding_id.0))
    };
    let find_expression_binding = || {
        if let Some(binding_id) = local_naming_result
            .expression_resolutions
            .get(&expression_id)
        {
            let binding_document_id = local_naming_result.bindings.get(binding_id)?.module_id;
            return Some((binding_document_id, *binding_id));
        }

        let symbol = *local_naming_result.non_locals.get(&expression_id)?;
        let export_document_id = *global_naming_result.global_bindings.get(&symbol)?;
        let export_document_naming = all_local_naming.get(&export_document_id)?;
        let export_module = analysis.module(export_document_id)?;
        find_exported_binding(export_module, export_document_naming, symbol)
            .map(|binding_id| (export_document_id, binding_id))
    };
    let render_nested = |nested_expression_id, nested_indent, lines: &mut Vec<String>| {
        render_named_expression(
            analysis,
            document_id,
            arena,
            nested_expression_id,
            local_naming_result,
            all_local_naming,
            global_naming_result,
            binding_display_labels,
            interner,
            nested_indent,
            lines,
        );
    };
    let render_arguments = |arguments: &[analysis::hir::Argument], lines: &mut Vec<String>| {
        for argument in arguments {
            let argument_prefix = "  ".repeat(indent + 1);
            if let Some(name) = argument.name {
                let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                lines.push(format!("{argument_prefix}Argument({rendered_name})"));
            } else {
                lines.push(format!("{argument_prefix}Argument"));
            }
            render_nested(argument.expression, indent + 2, lines);
        }
    };

    match &expression.kind {
        ExpressionKind::Null => lines.push(format!("{prefix}Null")),
        ExpressionKind::Logical(value) => lines.push(format!("{prefix}Logical({value})")),
        ExpressionKind::Integer(value) => lines.push(format!("{prefix}Integer({value})")),
        ExpressionKind::Double(value) => lines.push(format!("{prefix}Double({value})")),
        ExpressionKind::Character(value) => lines.push(format!("{prefix}Character({value:?})")),
        ExpressionKind::AtomicConstant(atomic) => {
            lines.push(format!("{prefix}AtomicConstant({atomic:?})"))
        }
        ExpressionKind::StringLiteralName(symbol) => {
            let name = interner.resolve(*symbol).unwrap_or("<unknown>");
            lines.push(format!("{prefix}StringLiteralName({name:?})"));
        }
        ExpressionKind::Symbol(symbol) => {
            let name = interner.resolve(*symbol).unwrap_or("<unknown>");
            let binding = find_expression_binding()
                .map(|(binding_document_id, binding_id)| {
                    render_binding_label(binding_document_id, binding_id)
                })
                .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}Symbol({name}@{binding})"));
        }
        ExpressionKind::Block { expressions, .. } => {
            lines.push(format!("{prefix}Block"));
            for nested_expression in expressions {
                render_nested(*nested_expression, indent + 1, lines);
            }
        }
        ExpressionKind::Assign { target, value, .. } => {
            let name = match target {
                analysis::hir::AssignTarget::Variable { symbol, .. } => {
                    interner.resolve(*symbol).unwrap_or("<unknown>")
                }
                analysis::hir::AssignTarget::Replacement { lhs } => {
                    analysis::hir::replacement_base(arena, *lhs)
                        .and_then(|(_, symbol)| interner.resolve(symbol))
                        .unwrap_or("<replacement>")
                }
            };
            let binding = find_expression_binding()
                .map(|(binding_document_id, binding_id)| {
                    render_binding_label(binding_document_id, binding_id)
                })
                .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}Assign({name}@{binding})"));
            render_nested(*value, indent + 1, lines);
        }
        ExpressionKind::Function { parameters, body } => {
            let rendered_parameters = parameters
                .iter()
                .map(|parameter| {
                    let name = interner.resolve(parameter.symbol).unwrap_or("<unknown>");
                    let binding = find_binding_by_symbol_and_range(
                        local_naming_result,
                        document_id,
                        parameter.symbol,
                        parameter.range,
                    )
                    .map(|binding_id| render_binding_label(document_id, binding_id))
                    .unwrap_or_else(|| "?".to_owned());
                    format!("{name}@{binding}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("{prefix}Function({rendered_parameters})"));
            render_nested(*body, indent + 1, lines);
        }
        ExpressionKind::Local { body } => {
            lines.push(format!("{prefix}Local"));
            render_nested(*body, indent + 1, lines);
        }
        ExpressionKind::If {
            condition,
            consequence,
            alternative,
        } => {
            lines.push(format!("{prefix}If"));
            render_nested(*condition, indent + 1, lines);
            render_nested(*consequence, indent + 1, lines);
            if let Some(alternative) = alternative {
                render_nested(*alternative, indent + 1, lines);
            }
        }
        ExpressionKind::For {
            variable,
            sequence,
            body,
        } => {
            let name = interner.resolve(*variable).unwrap_or("<unknown>");
            let binding = find_binding_by_symbol_and_range(
                local_naming_result,
                document_id,
                *variable,
                expression.range,
            )
            .map(|binding_id| render_binding_label(document_id, binding_id))
            .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}For({name}@{binding})"));
            render_nested(*sequence, indent + 1, lines);
            render_nested(*body, indent + 1, lines);
        }
        ExpressionKind::While { condition, body } => {
            lines.push(format!("{prefix}While"));
            render_nested(*condition, indent + 1, lines);
            render_nested(*body, indent + 1, lines);
        }
        ExpressionKind::Repeat { body } => {
            lines.push(format!("{prefix}Repeat"));
            render_nested(*body, indent + 1, lines);
        }
        ExpressionKind::UnaryMinus { value } => {
            lines.push(format!("{prefix}UnaryMinus"));
            render_nested(*value, indent + 1, lines);
        }
        ExpressionKind::UnaryNot { value } => {
            lines.push(format!("{prefix}UnaryNot"));
            render_nested(*value, indent + 1, lines);
        }
        ExpressionKind::Call { callee, arguments } => {
            lines.push(format!("{prefix}Call"));
            render_nested(*callee, indent + 1, lines);
            render_arguments(arguments, lines);
        }
        ExpressionKind::Subset { value, arguments } => {
            lines.push(format!("{prefix}Subset"));
            render_nested(*value, indent + 1, lines);
            render_arguments(arguments, lines);
        }
        ExpressionKind::Subset2 { value, arguments } => {
            lines.push(format!("{prefix}Subset2"));
            render_nested(*value, indent + 1, lines);
            render_arguments(arguments, lines);
        }
        ExpressionKind::Dollar { value, name } => {
            let rendered_name = interner.resolve(*name).unwrap_or("<unknown>");
            lines.push(format!("{prefix}Dollar({rendered_name})"));
            render_nested(*value, indent + 1, lines);
        }
        ExpressionKind::Break => lines.push(format!("{prefix}Break")),
        ExpressionKind::Next => lines.push(format!("{prefix}Next")),
        ExpressionKind::Unsupported => lines.push(format!("{prefix}Unsupported")),
    }
}

fn render_locally_named_expression(
    module_id: ModuleId,
    arena: &HirArena,
    expression_id: ExpressionId,
    local_naming_result: &NamesLocal,
    interner: &Interner,
    indent: usize,
    lines: &mut Vec<String>,
) {
    let expression = arena.get(expression_id);
    let prefix = "  ".repeat(indent);
    let render_binding_label = |binding_id: BindingId| format!("b{}", binding_id.0);
    let render_nested = |nested_expression_id, nested_indent, lines: &mut Vec<String>| {
        render_locally_named_expression(
            module_id,
            arena,
            nested_expression_id,
            local_naming_result,
            interner,
            nested_indent,
            lines,
        );
    };
    let render_arguments = |arguments: &[analysis::hir::Argument], lines: &mut Vec<String>| {
        for argument in arguments {
            let argument_prefix = "  ".repeat(indent + 1);
            if let Some(name) = argument.name {
                let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                lines.push(format!("{argument_prefix}Argument({rendered_name})"));
            } else {
                lines.push(format!("{argument_prefix}Argument"));
            }
            render_nested(argument.expression, indent + 2, lines);
        }
    };

    match &expression.kind {
        ExpressionKind::Null => lines.push(format!("{prefix}Null")),
        ExpressionKind::Logical(value) => lines.push(format!("{prefix}Logical({value})")),
        ExpressionKind::Integer(value) => lines.push(format!("{prefix}Integer({value})")),
        ExpressionKind::Double(value) => lines.push(format!("{prefix}Double({value})")),
        ExpressionKind::Character(value) => lines.push(format!("{prefix}Character({value:?})")),
        ExpressionKind::AtomicConstant(atomic) => {
            lines.push(format!("{prefix}AtomicConstant({atomic:?})"))
        }
        ExpressionKind::StringLiteralName(symbol) => {
            let name = interner.resolve(*symbol).unwrap_or("<unknown>");
            lines.push(format!("{prefix}StringLiteralName({name:?})"));
        }
        ExpressionKind::Symbol(symbol) => {
            let name = interner.resolve(*symbol).unwrap_or("<unknown>");
            let binding = local_naming_result
                .expression_resolutions
                .get(&expression_id)
                .map(|binding_id| render_binding_label(*binding_id))
                .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}Symbol({name}@{binding})"));
        }
        ExpressionKind::Block { expressions, .. } => {
            lines.push(format!("{prefix}Block"));
            for nested_expression in expressions {
                render_nested(*nested_expression, indent + 1, lines);
            }
        }
        ExpressionKind::Assign { target, value, .. } => {
            let name = match target {
                analysis::hir::AssignTarget::Variable { symbol, .. } => {
                    interner.resolve(*symbol).unwrap_or("<unknown>")
                }
                analysis::hir::AssignTarget::Replacement { lhs } => {
                    analysis::hir::replacement_base(arena, *lhs)
                        .and_then(|(_, symbol)| interner.resolve(symbol))
                        .unwrap_or("<replacement>")
                }
            };
            let binding = local_naming_result
                .expression_resolutions
                .get(&expression_id)
                .map(|binding_id| render_binding_label(*binding_id))
                .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}Assign({name}@{binding})"));
            render_nested(*value, indent + 1, lines);
        }
        ExpressionKind::Function { parameters, body } => {
            let rendered_parameters = parameters
                .iter()
                .map(|parameter| {
                    let name = interner.resolve(parameter.symbol).unwrap_or("<unknown>");
                    let binding = find_binding_by_symbol_and_range(
                        local_naming_result,
                        module_id,
                        parameter.symbol,
                        parameter.range,
                    )
                    .map(render_binding_label)
                    .unwrap_or_else(|| "?".to_owned());
                    format!("{name}@{binding}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("{prefix}Function({rendered_parameters})"));
            render_nested(*body, indent + 1, lines);
        }
        ExpressionKind::Local { body } => {
            lines.push(format!("{prefix}Local"));
            render_nested(*body, indent + 1, lines);
        }
        ExpressionKind::If {
            condition,
            consequence,
            alternative,
        } => {
            lines.push(format!("{prefix}If"));
            render_nested(*condition, indent + 1, lines);
            render_nested(*consequence, indent + 1, lines);
            if let Some(alternative) = alternative {
                render_nested(*alternative, indent + 1, lines);
            }
        }
        ExpressionKind::For {
            variable,
            sequence,
            body,
        } => {
            let name = interner.resolve(*variable).unwrap_or("<unknown>");
            let binding = find_binding_by_symbol_and_range(
                local_naming_result,
                module_id,
                *variable,
                expression.range,
            )
            .map(render_binding_label)
            .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}For({name}@{binding})"));
            render_nested(*sequence, indent + 1, lines);
            render_nested(*body, indent + 1, lines);
        }
        ExpressionKind::While { condition, body } => {
            lines.push(format!("{prefix}While"));
            render_nested(*condition, indent + 1, lines);
            render_nested(*body, indent + 1, lines);
        }
        ExpressionKind::Repeat { body } => {
            lines.push(format!("{prefix}Repeat"));
            render_nested(*body, indent + 1, lines);
        }
        ExpressionKind::UnaryMinus { value } => {
            lines.push(format!("{prefix}UnaryMinus"));
            render_nested(*value, indent + 1, lines);
        }
        ExpressionKind::UnaryNot { value } => {
            lines.push(format!("{prefix}UnaryNot"));
            render_nested(*value, indent + 1, lines);
        }
        ExpressionKind::Call { callee, arguments } => {
            lines.push(format!("{prefix}Call"));
            render_nested(*callee, indent + 1, lines);
            render_arguments(arguments, lines);
        }
        ExpressionKind::Subset { value, arguments } => {
            lines.push(format!("{prefix}Subset"));
            render_nested(*value, indent + 1, lines);
            render_arguments(arguments, lines);
        }
        ExpressionKind::Subset2 { value, arguments } => {
            lines.push(format!("{prefix}Subset2"));
            render_nested(*value, indent + 1, lines);
            render_arguments(arguments, lines);
        }
        ExpressionKind::Dollar { value, name } => {
            let rendered_name = interner.resolve(*name).unwrap_or("<unknown>");
            lines.push(format!("{prefix}Dollar({rendered_name})"));
            render_nested(*value, indent + 1, lines);
        }
        ExpressionKind::Break => lines.push(format!("{prefix}Break")),
        ExpressionKind::Next => lines.push(format!("{prefix}Next")),
        ExpressionKind::Unsupported => lines.push(format!("{prefix}Unsupported")),
    }
}

fn find_binding_by_symbol_and_range(
    local_naming_result: &NamesLocal,
    document_id: DocumentId,
    symbol: analysis::Symbol,
    range: tree_sitter::Range,
) -> Option<BindingId> {
    find_binding(local_naming_result, document_id, symbol, range)
}
