use {
    crate::hir::{DefinitionId, DefinitionItem, ExpressionId, ExpressionKind, HirArena, Module},
    std::{collections::HashMap, path::PathBuf},
};

pub(crate) struct RemappedModules {
    pub arena: HirArena,
    pub definitions: Vec<DefinitionItem>,
    pub expressions: Vec<ExpressionId>,
}

pub(crate) fn remap_package_modules(modules: &HashMap<PathBuf, Module>) -> RemappedModules {
    remap_modules_into_shared_package_arena(sorted_modules(modules))
}

pub(crate) fn sorted_modules(modules: &HashMap<PathBuf, Module>) -> Vec<(PathBuf, Module)> {
    let mut sorted_modules = modules
        .iter()
        .map(|(path, module)| (path.clone(), module.clone()))
        .collect::<Vec<_>>();
    sorted_modules.sort_by(|(left_path, _), (right_path, _)| left_path.cmp(right_path));
    sorted_modules
}

fn remap_modules_into_shared_package_arena(modules: Vec<(PathBuf, Module)>) -> RemappedModules {
    let mut arena = HirArena::new();
    let mut definitions = Vec::new();
    let mut expressions = Vec::new();
    let mut next_expression_id = 0u32;
    let mut next_definition_id = 0u32;
    let mut remapped_module_items = Vec::new();

    for (path, module) in modules {
        let expression_offset = next_expression_id;
        let definition_offset = next_definition_id;

        let remapped_expressions = module
            .arena
            .expressions()
            .iter()
            .cloned()
            .map(|mut expression| {
                expression.id = ExpressionId(expression.id.0 + expression_offset);
                remap_expression_kind(&mut expression.kind, expression_offset);
                expression
            })
            .collect::<Vec<_>>();
        next_expression_id +=
            u32::try_from(remapped_expressions.len()).expect("expression count exceeded u32");
        arena.expressions.extend(remapped_expressions.clone());

        let remapped_definitions = module
            .definitions
            .into_iter()
            .map(|definition| {
                DefinitionItem::new(
                    DefinitionId(definition.id.0 + definition_offset),
                    definition.range,
                    definition.definition,
                )
            })
            .collect::<Vec<_>>();
        next_definition_id +=
            u32::try_from(remapped_definitions.len()).expect("definition count exceeded u32");
        definitions.extend(remapped_definitions.clone());

        let remapped_module_expressions = module
            .expressions
            .into_iter()
            .map(|expression_id| ExpressionId(expression_id.0 + expression_offset))
            .collect::<Vec<_>>();
        expressions.extend(remapped_module_expressions.iter().copied());
        remapped_module_items.push((path, remapped_definitions, remapped_module_expressions));
    }

    let _remapped_modules = remapped_module_items
        .into_iter()
        .map(|(path, module_definitions, module_expressions)| {
            (
                path,
                Module::new(arena.clone(), module_definitions, module_expressions),
            )
        })
        .collect::<HashMap<_, _>>();

    RemappedModules {
        arena,
        definitions,
        expressions,
    }
}

fn remap_expression_kind(expression_kind: &mut ExpressionKind, expression_offset: u32) {
    match expression_kind {
        ExpressionKind::Block { expressions, .. } => {
            for expression_id in expressions {
                *expression_id = ExpressionId(expression_id.0 + expression_offset);
            }
        }
        ExpressionKind::Assign { value, .. }
        | ExpressionKind::UnaryMinus { value }
        | ExpressionKind::Dollar { value, .. } => {
            *value = ExpressionId(value.0 + expression_offset);
        }
        ExpressionKind::Function { body, .. } | ExpressionKind::Repeat { body } => {
            *body = ExpressionId(body.0 + expression_offset);
        }
        ExpressionKind::While { condition, body } => {
            *condition = ExpressionId(condition.0 + expression_offset);
            *body = ExpressionId(body.0 + expression_offset);
        }
        ExpressionKind::For { sequence, body, .. } => {
            *sequence = ExpressionId(sequence.0 + expression_offset);
            *body = ExpressionId(body.0 + expression_offset);
        }
        ExpressionKind::If {
            condition,
            consequence,
            alternative,
        } => {
            *condition = ExpressionId(condition.0 + expression_offset);
            *consequence = ExpressionId(consequence.0 + expression_offset);
            if let Some(alternative) = alternative {
                *alternative = ExpressionId(alternative.0 + expression_offset);
            }
        }
        ExpressionKind::Call { callee, arguments }
        | ExpressionKind::Subset {
            value: callee,
            arguments,
        }
        | ExpressionKind::Subset2 {
            value: callee,
            arguments,
        } => {
            *callee = ExpressionId(callee.0 + expression_offset);
            for argument in arguments {
                argument.expression = ExpressionId(argument.expression.0 + expression_offset);
            }
        }
        ExpressionKind::Null
        | ExpressionKind::Logical(_)
        | ExpressionKind::Integer(_)
        | ExpressionKind::Double(_)
        | ExpressionKind::Character(_)
        | ExpressionKind::StringLiteralName(_)
        | ExpressionKind::Symbol(_)
        | ExpressionKind::Unsupported => {}
    }
}
