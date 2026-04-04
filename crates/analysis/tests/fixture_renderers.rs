use analysis::{
    Interner,
    hir::{
        DefinitionItem, DefinitionKind, ExpressionId, ExpressionKind, HirArena, Module, ModuleId,
    },
    lower::LoweringContext,
    naming::{BindingId, ExpressionKey, NamesLocal, NamesGlobal, ProvisionalBindingId},
    type_syntax::render_surface_type,
    typecheck::{InferenceError, InferenceState},
    types::{Atomic, CoreType, InferenceVariableId, TypeScheme},
};

pub fn render_expression_error_kind(error: &InferenceError) -> &'static str {
    match error {
        InferenceError::UnknownInferenceVariable(_) => "error: unknown inference variable",
        InferenceError::UnknownName { .. } => "error: unknown name",
        InferenceError::ExpectedFunction { .. } => "error: expected function",
        InferenceError::OccursCheckFailed { .. } => "error: occurs check failed",
        InferenceError::TypeMismatch { .. } => "error: type mismatch",
        InferenceError::InvalidPlusOperand { .. } => "error: invalid plus operand",
        InferenceError::TupleLengthMismatch { .. } => "error: tuple length mismatch",
        InferenceError::MixedListElements { .. } => "error: mixed list elements",
        InferenceError::RecordFieldMismatch { .. } => "error: record field mismatch",
        InferenceError::FunctionArityMismatch { .. } => "error: function arity mismatch",
        InferenceError::NamedParameterMismatch { .. } => "error: named parameter mismatch",
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
        let mut renderer = SimpleTypeRenderer::new(lowering_context.interner());
        lines.push(renderer.render(&resolved_type));
    }

    lines.join("\n")
}

pub fn render_core_type(interner: &Interner, core_type: &CoreType) -> String {
    let mut renderer = SimpleTypeRenderer::new(interner);
    renderer.render(core_type)
}

pub fn render_type_scheme(interner: &Interner, type_scheme: &TypeScheme) -> String {
    let mut renderer = SimpleTypeRenderer::new(interner);
    renderer.render_type_scheme(type_scheme)
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
        if let ExpressionKind::Assign { target, .. } = &expression.kind {
            let name = lowering_context
                .interner()
                .resolve(*target)
                .unwrap_or("<unknown>");
            let binding = inference_state
                .lookup_name(*target)
                .unwrap_or_else(|| panic!("binding `{name}` should be present after inference"));
            let mut renderer = SimpleTypeRenderer::new(lowering_context.interner());
            exported_entries.push((
                definition_count + expression_index,
                *target,
                format!(
                    "{name}: {}",
                    renderer.render_type_scheme(&binding.type_scheme)
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

pub fn render_named_hir(
    module_id: ModuleId,
    module: &Module,
    naming_result: &NamesGlobal,
    interner: &Interner,
) -> String {
    let mut lines = Vec::new();

    for definition in &module.definitions {
        render_named_definition(definition, interner, 0, &mut lines);
    }

    for expression_id in &module.expressions {
        render_named_expression(
            module_id,
            &module.arena,
            *expression_id,
            naming_result,
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

struct SimpleTypeRenderer<'a> {
    interner: &'a Interner,
    variable_names: std::collections::BTreeMap<InferenceVariableId, String>,
    quantified_variable_names: std::collections::BTreeMap<InferenceVariableId, String>,
    next_variable_index: usize,
}

impl<'a> SimpleTypeRenderer<'a> {
    fn new(interner: &'a Interner) -> Self {
        Self {
            interner,
            variable_names: std::collections::BTreeMap::new(),
            quantified_variable_names: std::collections::BTreeMap::new(),
            next_variable_index: 0,
        }
    }

    fn render_type_scheme(&mut self, type_scheme: &TypeScheme) -> String {
        let quantified_names = type_scheme
            .quantified_variables
            .iter()
            .enumerate()
            .map(|(index, variable)| {
                let name = quantified_variable_name(index);
                self.quantified_variable_names
                    .insert(*variable, name.clone());
                name
            })
            .collect::<Vec<_>>();
        let rendered_body = self.render(&type_scheme.body);

        if quantified_names.is_empty() {
            rendered_body
        } else {
            format!("<{}> {}", quantified_names.join(", "), rendered_body)
        }
    }

    fn render(&mut self, core_type: &CoreType) -> String {
        match core_type {
            CoreType::Any => "Any".to_owned(),
            CoreType::Unknown => "Unknown".to_owned(),
            CoreType::Null => "NULL".to_owned(),
            CoreType::Nullable(inner_type) => format!("{} | NULL", self.render(inner_type)),
            CoreType::Scalar(atomic) => render_atomic(*atomic).to_owned(),
            CoreType::Vector(atomic) => format!("{}[]", render_atomic(*atomic)),
            CoreType::NamedVector(atomic) => format!("{}[named]", render_atomic(*atomic)),
            CoreType::List(item_type) => format!("list[{}]", self.render(item_type)),
            CoreType::NamedList(item_type) => format!("list[named: {}]", self.render(item_type)),
            CoreType::Record(fields) => {
                let rendered_fields = fields
                    .iter()
                    .map(|field| {
                        let name = self.interner.resolve(field.name).unwrap_or("<unknown>");
                        format!("{name}: {}", self.render(&field.value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("list{{{rendered_fields}}}")
            }
            CoreType::Tuple(items) => {
                let rendered_items = items
                    .iter()
                    .map(|item| self.render(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("list{{{rendered_items}}}")
            }
            CoreType::Function(function_type) => {
                let rendered_parameters = function_type
                    .parameters
                    .iter()
                    .map(|parameter| self.render(parameter))
                    .collect::<Vec<_>>();
                let rendered_named_parameters = function_type
                    .named_parameters
                    .iter()
                    .map(|parameter| {
                        let name = self.interner.resolve(parameter.name).unwrap_or("<unknown>");
                        let rendered_name = if parameter.optional {
                            format!("[{name}]")
                        } else {
                            name.to_owned()
                        };
                        format!("{rendered_name}: {}", self.render(&parameter.value))
                    })
                    .collect::<Vec<_>>();
                let mut rendered_parts = rendered_parameters;
                rendered_parts.extend(rendered_named_parameters);
                format!(
                    "fn({}) -> {}",
                    rendered_parts.join(", "),
                    self.render(&function_type.return_type)
                )
            }
            CoreType::Variable(variable) => self.render_variable(*variable),
        }
    }

    fn render_variable(&mut self, variable: InferenceVariableId) -> String {
        if let Some(name) = self.quantified_variable_names.get(&variable) {
            return name.clone();
        }

        if !self.variable_names.contains_key(&variable) {
            let name = format!("?{}", self.next_variable_index + 1);
            self.next_variable_index += 1;
            self.variable_names.insert(variable, name);
        }

        self.variable_names
            .get(&variable)
            .cloned()
            .unwrap_or_else(|| "?".to_owned())
    }
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

fn render_named_expression(
    module_id: ModuleId,
    arena: &HirArena,
    expression_id: ExpressionId,
    naming_result: &NamesGlobal,
    interner: &Interner,
    indent: usize,
    lines: &mut Vec<String>,
) {
    let expression = arena.get(expression_id);
    let prefix = "  ".repeat(indent);

    match &expression.kind {
        ExpressionKind::Null => lines.push(format!("{prefix}Null")),
        ExpressionKind::Logical(value) => lines.push(format!("{prefix}Logical({value})")),
        ExpressionKind::Integer(value) => lines.push(format!("{prefix}Integer({value})")),
        ExpressionKind::Double(value) => lines.push(format!("{prefix}Double({value})")),
        ExpressionKind::Character(value) => lines.push(format!("{prefix}Character({value:?})")),
        ExpressionKind::StringLiteralName(symbol) => {
            let name = interner.resolve(*symbol).unwrap_or("<unknown>");
            lines.push(format!("{prefix}StringLiteralName({name:?})"));
        }
        ExpressionKind::Symbol(symbol) => {
            let name = interner.resolve(*symbol).unwrap_or("<unknown>");
            let binding = naming_result
                .resolutions
                .get(&ExpressionKey {
                    module_id,
                    expression_id,
                })
                .map(|binding_id| binding_label(*binding_id))
                .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}Symbol({name}@{binding})"));
        }
        ExpressionKind::Block { expressions, .. } => {
            lines.push(format!("{prefix}Block"));
            for nested_expression in expressions {
                render_named_expression(
                    module_id,
                    arena,
                    *nested_expression,
                    naming_result,
                    interner,
                    indent + 1,
                    lines,
                );
            }
        }
        ExpressionKind::Assign { target, value, .. } => {
            let name = interner.resolve(*target).unwrap_or("<unknown>");
            let binding = naming_result
                .resolutions
                .get(&ExpressionKey {
                    module_id,
                    expression_id,
                })
                .map(|binding_id| binding_label(*binding_id))
                .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}Assign({name}@{binding})"));
            render_named_expression(
                module_id,
                arena,
                *value,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::Function { parameters, body } => {
            let rendered_parameters = parameters
                .iter()
                .map(|parameter| {
                    let name = interner.resolve(parameter.symbol).unwrap_or("<unknown>");
                    let binding = find_binding_by_symbol_and_range(
                        naming_result,
                        module_id,
                        parameter.symbol,
                        parameter.range,
                    )
                    .map(binding_label)
                    .unwrap_or_else(|| "?".to_owned());
                    format!("{name}@{binding}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("{prefix}Function({rendered_parameters})"));
            render_named_expression(
                module_id,
                arena,
                *body,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::If {
            condition,
            consequence,
            alternative,
        } => {
            lines.push(format!("{prefix}If"));
            render_named_expression(
                module_id,
                arena,
                *condition,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
            render_named_expression(
                module_id,
                arena,
                *consequence,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
            if let Some(alternative) = alternative {
                render_named_expression(
                    module_id,
                    arena,
                    *alternative,
                    naming_result,
                    interner,
                    indent + 1,
                    lines,
                );
            }
        }
        ExpressionKind::For {
            variable,
            sequence,
            body,
        } => {
            let name = interner.resolve(*variable).unwrap_or("<unknown>");
            let binding = find_binding_by_symbol_and_range(
                naming_result,
                module_id,
                *variable,
                expression.range,
            )
            .map(binding_label)
            .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}For({name}@{binding})"));
            render_named_expression(
                module_id,
                arena,
                *sequence,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
            render_named_expression(
                module_id,
                arena,
                *body,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::While { condition, body } => {
            lines.push(format!("{prefix}While"));
            render_named_expression(
                module_id,
                arena,
                *condition,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
            render_named_expression(
                module_id,
                arena,
                *body,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::Repeat { body } => {
            lines.push(format!("{prefix}Repeat"));
            render_named_expression(
                module_id,
                arena,
                *body,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::UnaryMinus { value } => {
            lines.push(format!("{prefix}UnaryMinus"));
            render_named_expression(
                module_id,
                arena,
                *value,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::Call { callee, arguments } => {
            lines.push(format!("{prefix}Call"));
            render_named_expression(
                module_id,
                arena,
                *callee,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
            for argument in arguments {
                let argument_prefix = "  ".repeat(indent + 1);
                if let Some(name) = argument.name {
                    let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                    lines.push(format!("{argument_prefix}Argument({rendered_name})"));
                } else {
                    lines.push(format!("{argument_prefix}Argument"));
                }
                render_named_expression(
                    module_id,
                    arena,
                    argument.expression,
                    naming_result,
                    interner,
                    indent + 2,
                    lines,
                );
            }
        }
        ExpressionKind::Subset { value, arguments } => {
            lines.push(format!("{prefix}Subset"));
            render_named_expression(
                module_id,
                arena,
                *value,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
            for argument in arguments {
                let argument_prefix = "  ".repeat(indent + 1);
                if let Some(name) = argument.name {
                    let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                    lines.push(format!("{argument_prefix}Argument({rendered_name})"));
                } else {
                    lines.push(format!("{argument_prefix}Argument"));
                }
                render_named_expression(
                    module_id,
                    arena,
                    argument.expression,
                    naming_result,
                    interner,
                    indent + 2,
                    lines,
                );
            }
        }
        ExpressionKind::Subset2 { value, arguments } => {
            lines.push(format!("{prefix}Subset2"));
            render_named_expression(
                module_id,
                arena,
                *value,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
            for argument in arguments {
                let argument_prefix = "  ".repeat(indent + 1);
                if let Some(name) = argument.name {
                    let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                    lines.push(format!("{argument_prefix}Argument({rendered_name})"));
                } else {
                    lines.push(format!("{argument_prefix}Argument"));
                }
                render_named_expression(
                    module_id,
                    arena,
                    argument.expression,
                    naming_result,
                    interner,
                    indent + 2,
                    lines,
                );
            }
        }
        ExpressionKind::Dollar { value, name } => {
            let rendered_name = interner.resolve(*name).unwrap_or("<unknown>");
            lines.push(format!("{prefix}Dollar({rendered_name})"));
            render_named_expression(
                module_id,
                arena,
                *value,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
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

    match &expression.kind {
        ExpressionKind::Null => lines.push(format!("{prefix}Null")),
        ExpressionKind::Logical(value) => lines.push(format!("{prefix}Logical({value})")),
        ExpressionKind::Integer(value) => lines.push(format!("{prefix}Integer({value})")),
        ExpressionKind::Double(value) => lines.push(format!("{prefix}Double({value})")),
        ExpressionKind::Character(value) => lines.push(format!("{prefix}Character({value:?})")),
        ExpressionKind::StringLiteralName(symbol) => {
            let name = interner.resolve(*symbol).unwrap_or("<unknown>");
            lines.push(format!("{prefix}StringLiteralName({name:?})"));
        }
        ExpressionKind::Symbol(symbol) => {
            let name = interner.resolve(*symbol).unwrap_or("<unknown>");
            let binding = local_naming_result
                .expression_resolutions
                .get(&expression_id)
                .map(|binding_id| provisional_binding_label(*binding_id))
                .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}Symbol({name}@{binding})"));
        }
        ExpressionKind::Block { expressions, .. } => {
            lines.push(format!("{prefix}Block"));
            for nested_expression in expressions {
                render_locally_named_expression(
                    module_id,
                    arena,
                    *nested_expression,
                    local_naming_result,
                    interner,
                    indent + 1,
                    lines,
                );
            }
        }
        ExpressionKind::Assign { target, value, .. } => {
            let name = interner.resolve(*target).unwrap_or("<unknown>");
            let binding = local_naming_result
                .expression_resolutions
                .get(&expression_id)
                .map(|binding_id| provisional_binding_label(*binding_id))
                .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}Assign({name}@{binding})"));
            render_locally_named_expression(
                module_id,
                arena,
                *value,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::Function { parameters, body } => {
            let rendered_parameters = parameters
                .iter()
                .map(|parameter| {
                    let name = interner.resolve(parameter.symbol).unwrap_or("<unknown>");
                    let binding = find_local_binding_by_symbol_and_range(
                        local_naming_result,
                        module_id,
                        parameter.symbol,
                        parameter.range,
                    )
                    .map(provisional_binding_label)
                    .unwrap_or_else(|| "?".to_owned());
                    format!("{name}@{binding}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("{prefix}Function({rendered_parameters})"));
            render_locally_named_expression(
                module_id,
                arena,
                *body,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::If {
            condition,
            consequence,
            alternative,
        } => {
            lines.push(format!("{prefix}If"));
            render_locally_named_expression(
                module_id,
                arena,
                *condition,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
            render_locally_named_expression(
                module_id,
                arena,
                *consequence,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
            if let Some(alternative) = alternative {
                render_locally_named_expression(
                    module_id,
                    arena,
                    *alternative,
                    local_naming_result,
                    interner,
                    indent + 1,
                    lines,
                );
            }
        }
        ExpressionKind::For {
            variable,
            sequence,
            body,
        } => {
            let name = interner.resolve(*variable).unwrap_or("<unknown>");
            let binding = find_local_binding_by_symbol_and_range(
                local_naming_result,
                module_id,
                *variable,
                expression.range,
            )
            .map(provisional_binding_label)
            .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}For({name}@{binding})"));
            render_locally_named_expression(
                module_id,
                arena,
                *sequence,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
            render_locally_named_expression(
                module_id,
                arena,
                *body,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::While { condition, body } => {
            lines.push(format!("{prefix}While"));
            render_locally_named_expression(
                module_id,
                arena,
                *condition,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
            render_locally_named_expression(
                module_id,
                arena,
                *body,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::Repeat { body } => {
            lines.push(format!("{prefix}Repeat"));
            render_locally_named_expression(
                module_id,
                arena,
                *body,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::UnaryMinus { value } => {
            lines.push(format!("{prefix}UnaryMinus"));
            render_locally_named_expression(
                module_id,
                arena,
                *value,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::Call { callee, arguments } => {
            lines.push(format!("{prefix}Call"));
            render_locally_named_expression(
                module_id,
                arena,
                *callee,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
            for argument in arguments {
                let argument_prefix = "  ".repeat(indent + 1);
                if let Some(name) = argument.name {
                    let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                    lines.push(format!("{argument_prefix}Argument({rendered_name})"));
                } else {
                    lines.push(format!("{argument_prefix}Argument"));
                }
                render_locally_named_expression(
                    module_id,
                    arena,
                    argument.expression,
                    local_naming_result,
                    interner,
                    indent + 2,
                    lines,
                );
            }
        }
        ExpressionKind::Subset { value, arguments } => {
            lines.push(format!("{prefix}Subset"));
            render_locally_named_expression(
                module_id,
                arena,
                *value,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
            for argument in arguments {
                let argument_prefix = "  ".repeat(indent + 1);
                if let Some(name) = argument.name {
                    let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                    lines.push(format!("{argument_prefix}Argument({rendered_name})"));
                } else {
                    lines.push(format!("{argument_prefix}Argument"));
                }
                render_locally_named_expression(
                    module_id,
                    arena,
                    argument.expression,
                    local_naming_result,
                    interner,
                    indent + 2,
                    lines,
                );
            }
        }
        ExpressionKind::Subset2 { value, arguments } => {
            lines.push(format!("{prefix}Subset2"));
            render_locally_named_expression(
                module_id,
                arena,
                *value,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
            for argument in arguments {
                let argument_prefix = "  ".repeat(indent + 1);
                if let Some(name) = argument.name {
                    let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                    lines.push(format!("{argument_prefix}Argument({rendered_name})"));
                } else {
                    lines.push(format!("{argument_prefix}Argument"));
                }
                render_locally_named_expression(
                    module_id,
                    arena,
                    argument.expression,
                    local_naming_result,
                    interner,
                    indent + 2,
                    lines,
                );
            }
        }
        ExpressionKind::Dollar { value, name } => {
            let rendered_name = interner.resolve(*name).unwrap_or("<unknown>");
            lines.push(format!("{prefix}Dollar({rendered_name})"));
            render_locally_named_expression(
                module_id,
                arena,
                *value,
                local_naming_result,
                interner,
                indent + 1,
                lines,
            );
        }
        ExpressionKind::Unsupported => lines.push(format!("{prefix}Unsupported")),
    }
}

fn binding_label(binding_id: BindingId) -> String {
    format!("b{}", binding_id.0)
}

fn provisional_binding_label(binding_id: ProvisionalBindingId) -> String {
    format!("b{}", binding_id.0)
}

fn find_binding_by_symbol_and_range(
    naming_result: &NamesGlobal,
    module_id: ModuleId,
    symbol: analysis::Symbol,
    range: tree_sitter::Range,
) -> Option<BindingId> {
    naming_result
        .bindings
        .values()
        .find(|binding| {
            binding.module_id == module_id && binding.symbol == symbol && binding.range == range
        })
        .map(|binding| binding.id)
}

fn find_local_binding_by_symbol_and_range(
    local_naming_result: &NamesLocal,
    module_id: ModuleId,
    symbol: analysis::Symbol,
    range: tree_sitter::Range,
) -> Option<ProvisionalBindingId> {
    local_naming_result
        .bindings
        .iter()
        .find(|(_, binding)| {
            binding.module_id == module_id && binding.symbol == symbol && binding.range == range
        })
        .map(|(binding_id, _)| *binding_id)
}

fn render_atomic(atomic: Atomic) -> &'static str {
    match atomic {
        Atomic::Logical => "logical",
        Atomic::Integer => "integer",
        Atomic::Double => "double",
        Atomic::Complex => "complex",
        Atomic::Character => "character",
        Atomic::Raw => "raw",
    }
}

fn quantified_variable_name(index: usize) -> String {
    const QUANTIFIED_NAMES: [&str; 7] = ["T", "U", "V", "W", "X", "Y", "Z"];

    QUANTIFIED_NAMES
        .get(index)
        .map(|name| (*name).to_owned())
        .unwrap_or_else(|| format!("T{}", index + 1))
}
