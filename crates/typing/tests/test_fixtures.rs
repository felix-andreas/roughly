#[path = "fixture_renderers.rs"]
mod fixture_renderers;
#[path = "fixture_workspace.rs"]
mod fixture_workspace;

use {
    fixture_renderers::{
        render_core_type, render_diagnostics, render_expression_error_kind,
        render_expression_types, render_interface_snapshot, render_multi_file_output,
        render_named_hir, render_named_module, render_type_scheme,
    },
    fixture_workspace::{simple_fixture_source, with_fixture_document, with_fixture_documents},
    fixtures::{Fixture, FixtureKind, run_fixture_suite},
    std::{collections::BTreeMap, mem, path::PathBuf},
    typing::{
        AnalysisState, Interner,
        hir::{DefinitionId, DefinitionItem, ExpressionId, ExpressionKind, HirArena},
        lower::LoweringContext,
        naming::NamingContext,
        type_syntax::{parse_type_syntax, render_type_syntax},
        typecheck::{BuiltinKind, InferenceState},
    },
};

#[test]
fn bindings() {
    run_fixture_suite("tests/bindings", "bindings", run_bindings_fixture);
}

#[test]
fn diagnostics() {
    run_fixture_suite("tests/diagnostics", "diagnostics", run_diagnostics_fixture);
}

#[test]
fn environment() {
    run_fixture_suite("tests/environment", "environment", run_environment_fixture);
}

#[test]
fn expressions() {
    run_fixture_suite("tests/expressions", "expressions", run_expressions_fixture);
}

#[test]
fn generalization() {
    run_fixture_suite(
        "tests/generalization",
        "generalization",
        run_generalization_fixture,
    );
}

#[test]
fn instantiation() {
    run_fixture_suite(
        "tests/instantiation",
        "instantiation",
        run_instantiation_fixture,
    );
}

#[test]
fn interfaces() {
    run_fixture_suite("tests/interfaces", "interfaces", run_interfaces_fixture);
}

#[test]
fn lowering() {
    run_fixture_suite("tests/lowering", "lowering", run_lowering_fixture);
}

#[test]
fn naming() {
    run_fixture_suite("tests/naming", "naming", run_naming_fixture);
}

#[test]
fn substitution() {
    run_fixture_suite(
        "tests/substitution",
        "substitution",
        run_substitution_fixture,
    );
}

#[test]
fn type_syntax() {
    run_fixture_suite("tests/type_syntax", "type_syntax", run_type_syntax_fixture);
}

#[test]
fn unification() {
    run_fixture_suite("tests/unification", "unification", run_unification_fixture);
}

fn run_bindings_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = with_fixture_document(source, |document| {
        lowering_context.lower_root_with_rope(
            document.document().tree().root_node(),
            document.document().rope(),
        )
    })?;
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);
    let mut lines = Vec::new();

    for expression_id in &module.expressions {
        let expression = module.arena.get(*expression_id);
        inference_state
            .infer_expression(expression, &module.arena)
            .map_err(|error| render_expression_error_kind(&error).to_owned())?;

        if let ExpressionKind::Assign { target, .. } = &expression.kind {
            let name = lowering_context
                .interner()
                .resolve(*target)
                .unwrap_or("<unknown>");
            let binding = inference_state
                .lookup_name(*target)
                .unwrap_or_else(|| panic!("binding `{name}` should be present after inference"));
            lines.push(format!(
                "{name}: {}",
                render_type_scheme(lowering_context.interner(), &binding.type_scheme)
            ));
        }
    }

    Ok(lines.join("\n"))
}

fn run_diagnostics_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;
    let rendered = with_fixture_document(source, |document| {
        let mut analysis_state = AnalysisState::new();
        typing::check(
            document.document().tree().root_node(),
            document.document().rope(),
            &mut analysis_state,
        )
        .render(source)
    })?;
    Ok(rendered)
}

fn run_environment_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = with_fixture_document(source, |document| {
        lowering_context.lower_root_with_rope(
            document.document().tree().root_node(),
            document.document().rope(),
        )
    })?;
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);
    let mut lines = Vec::new();

    for expression_id in &module.expressions {
        let expression = module.arena.get(*expression_id);
        let inferred_type = inference_state
            .infer_expression(expression, &module.arena)
            .map_err(|error| render_expression_error_kind(&error).to_owned())?;

        match &expression.kind {
            ExpressionKind::Assign { target, .. } => {
                let name = lowering_context
                    .interner()
                    .resolve(*target)
                    .unwrap_or("<unknown>");
                let binding = inference_state.lookup_name(*target).unwrap_or_else(|| {
                    panic!("binding `{name}` should be present after inference")
                });
                lines.push(format!(
                    "{name}: {}",
                    render_type_scheme(lowering_context.interner(), &binding.type_scheme)
                ));
            }
            _ => {
                let resolved_type = inference_state
                    .resolve(inferred_type)
                    .map_err(|error| render_expression_error_kind(&error).to_owned())?;
                lines.push(render_core_type(
                    lowering_context.interner(),
                    &resolved_type,
                ));
            }
        }
    }

    Ok(lines.join("\n"))
}

fn run_expressions_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = with_fixture_document(source, |document| {
        lowering_context.lower_root_with_rope(
            document.document().tree().root_node(),
            document.document().rope(),
        )
    })?;
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);
    let inferred_types = match inference_state.infer_module(&module) {
        Ok(inferred_types) => inferred_types,
        Err(error) => return Ok(render_expression_error_kind(&error).to_owned()),
    };

    Ok(render_expression_types(
        &mut inference_state,
        &lowering_context,
        &inferred_types,
    ))
}

fn run_generalization_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = with_fixture_document(source, |document| {
        lowering_context.lower_root_with_rope(
            document.document().tree().root_node(),
            document.document().rope(),
        )
    })?;
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);
    let mut lines = Vec::new();

    for expression_id in &module.expressions {
        let expression = module.arena.get(*expression_id);
        inference_state
            .infer_expression(expression, &module.arena)
            .map_err(|error| render_expression_error_kind(&error).to_owned())?;

        if let ExpressionKind::Assign { target, .. } = &expression.kind {
            let name = lowering_context
                .interner()
                .resolve(*target)
                .unwrap_or("<unknown>");
            let binding = inference_state
                .lookup_name(*target)
                .unwrap_or_else(|| panic!("binding `{name}` should be present after inference"));
            lines.push(format!(
                "{name}: {}",
                render_type_scheme(lowering_context.interner(), &binding.type_scheme)
            ));
        }
    }

    Ok(lines.join("\n"))
}

fn run_instantiation_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = with_fixture_document(source, |document| {
        lowering_context.lower_root_with_rope(
            document.document().tree().root_node(),
            document.document().rope(),
        )
    })?;
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);
    let mut lines = Vec::new();

    for expression_id in &module.expressions {
        let expression = module.arena.get(*expression_id);
        let inferred_type = inference_state
            .infer_expression(expression, &module.arena)
            .map_err(|error| render_expression_error_kind(&error).to_owned())?;

        match &expression.kind {
            ExpressionKind::Assign { target, .. } => {
                let name = lowering_context
                    .interner()
                    .resolve(*target)
                    .unwrap_or("<unknown>");
                let binding = inference_state.lookup_name(*target).unwrap_or_else(|| {
                    panic!("binding `{name}` should be present after inference")
                });
                lines.push(format!(
                    "{name}: {}",
                    render_type_scheme(lowering_context.interner(), &binding.type_scheme)
                ));
            }
            _ => {
                let resolved_type = inference_state
                    .resolve(inferred_type)
                    .map_err(|error| render_expression_error_kind(&error).to_owned())?;
                lines.push(render_core_type(
                    lowering_context.interner(),
                    &resolved_type,
                ));
            }
        }
    }

    Ok(lines.join("\n"))
}

fn run_interfaces_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = with_fixture_document(source, |document| {
        lowering_context.lower_root_with_rope(
            document.document().tree().root_node(),
            document.document().rope(),
        )
    })?;
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);

    if inference_state.infer_module(&module).is_err() {
        return Ok("error: inference".to_owned());
    }

    Ok(render_interface_snapshot(
        &module,
        &inference_state,
        &lowering_context,
    ))
}

fn run_lowering_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;
    let (lowering_context, lowering_result) = with_fixture_document(source, |document| {
        let mut lowering_context = LoweringContext::new();
        let lowering_result = lowering_context.lower_root_with_rope_with_diagnostics(
            document.document().tree().root_node(),
            document.document().rope(),
        );
        (lowering_context, lowering_result)
    })?;

    if !lowering_result.diagnostics.is_empty() {
        return Ok(render_diagnostics(source, &lowering_result.diagnostics));
    }

    Ok(lowering_result.module.render(lowering_context.interner()))
}

fn run_naming_fixture(fixture: &Fixture) -> Result<String, String> {
    if let FixtureKind::MultiFile(case) = &fixture.kind {
        let lowered_project = with_fixture_documents(&case.input_files, |workspace| {
            let mut lowering_context = LoweringContext::new();
            let mut lowered_files = Vec::with_capacity(case.input_files.len());

            for input_file in &case.input_files {
                let document = workspace
                    .document(&input_file.path)
                    .ok_or_else(|| "workspace".to_owned())?;
                let lowering_result = lowering_context.lower_root_with_rope_with_diagnostics(
                    document.document().tree().root_node(),
                    document.document().rope(),
                );
                lowered_files.push((
                    input_file.path.clone(),
                    lowering_result.module,
                    lowering_result.diagnostics,
                ));
            }

            Ok((mem::take(lowering_context.interner_mut()), lowered_files))
        })?;

        if lowered_project
            .1
            .iter()
            .any(|(_, _, diagnostics)| !diagnostics.is_empty())
        {
            return Err("multi-file lowering".to_owned());
        }

        let mut arena = HirArena::new();
        let mut definitions = Vec::new();
        let mut expressions = Vec::new();
        let mut files_by_path =
            BTreeMap::<PathBuf, (Vec<DefinitionItem>, Vec<ExpressionId>)>::new();
        let mut next_expression_id = 0u32;
        let mut next_definition_id = 0u32;

        for (path, module, _) in lowered_project.1 {
            let expression_offset = next_expression_id;
            let definition_offset = next_definition_id;

            let remapped_arena_expressions = module
                .arena
                .expressions()
                .iter()
                .cloned()
                .map(|mut expression| {
                    expression.id = ExpressionId(expression.id.0 + expression_offset);
                    match &mut expression.kind {
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
                                argument.expression =
                                    ExpressionId(argument.expression.0 + expression_offset);
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
                    expression
                })
                .collect::<Vec<_>>();
            next_expression_id += u32::try_from(remapped_arena_expressions.len())
                .expect("expression count exceeded u32");
            arena.expressions.extend(remapped_arena_expressions);

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

            let remapped_expressions = module
                .expressions
                .into_iter()
                .map(|expression_id| ExpressionId(expression_id.0 + expression_offset))
                .collect::<Vec<_>>();

            definitions.extend(remapped_definitions.clone());
            expressions.extend(remapped_expressions.iter().copied());
            files_by_path.insert(path, (remapped_definitions, remapped_expressions));
        }

        let merged_module =
            typing::Module::new(arena.clone(), definitions.clone(), expressions.clone());
        let naming_result = NamingContext::new(&merged_module.arena, &lowered_project.0)
            .resolve_module(&merged_module);

        if !naming_result.diagnostics.is_empty() {
            return Err("multi-file naming".to_owned());
        }

        let mut rendered_outputs = Vec::with_capacity(case.expectations.len());
        for expectation in &case.expectations {
            let (file_definitions, file_expressions) = files_by_path
                .get(&expectation.path)
                .ok_or_else(|| "missing file".to_owned())?;
            rendered_outputs.push((
                expectation.path.clone(),
                render_named_hir(
                    &arena,
                    file_definitions,
                    file_expressions,
                    &naming_result,
                    &lowered_project.0,
                ),
            ));
        }

        return Ok(render_multi_file_output(&rendered_outputs));
    }

    let source = simple_fixture_source(fixture)?;
    let (lowering_context, lowering_result) = with_fixture_document(source, |document| {
        let mut lowering_context = LoweringContext::new();
        let lowering_result = lowering_context.lower_root_with_rope_with_diagnostics(
            document.document().tree().root_node(),
            document.document().rope(),
        );
        (lowering_context, lowering_result)
    })?;

    if !lowering_result.diagnostics.is_empty() {
        return Ok(render_diagnostics(source, &lowering_result.diagnostics));
    }

    let module = lowering_result.module;
    let naming_result =
        NamingContext::new(&module.arena, lowering_context.interner()).resolve_module(&module);
    if !naming_result.diagnostics.is_empty() {
        return Ok(render_diagnostics(source, &naming_result.diagnostics));
    }

    Ok(render_named_module(
        &module,
        &naming_result,
        lowering_context.interner(),
    ))
}

fn run_substitution_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = with_fixture_document(source, |document| {
        lowering_context.lower_root_with_rope(
            document.document().tree().root_node(),
            document.document().rope(),
        )
    })?;
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);
    let mut lines = Vec::new();

    for expression_id in &module.expressions {
        let expression = module.arena.get(*expression_id);
        let inferred_type = inference_state
            .infer_expression(expression, &module.arena)
            .map_err(|error| render_expression_error_kind(&error).to_owned())?;

        match &expression.kind {
            ExpressionKind::Assign { target, .. } => {
                let name = lowering_context
                    .interner()
                    .resolve(*target)
                    .unwrap_or("<unknown>");
                let binding = inference_state.lookup_name(*target).unwrap_or_else(|| {
                    panic!("binding `{name}` should be present after inference")
                });
                lines.push(format!(
                    "{name}: {}",
                    render_type_scheme(lowering_context.interner(), &binding.type_scheme)
                ));
            }
            _ => {
                let resolved_type = inference_state
                    .resolve(inferred_type)
                    .map_err(|error| render_expression_error_kind(&error).to_owned())?;
                lines.push(render_core_type(
                    lowering_context.interner(),
                    &resolved_type,
                ));
            }
        }
    }

    Ok(lines.join("\n"))
}

fn run_type_syntax_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;

    let mut interner = Interner::new();
    Ok(match parse_type_syntax(source, &mut interner) {
        Ok(item) => render_type_syntax(&item, &interner),
        Err(error) => format!("{error:?}"),
    })
}

fn run_unification_fixture(fixture: &Fixture) -> Result<String, String> {
    let source = simple_fixture_source(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = with_fixture_document(source, |document| {
        lowering_context.lower_root_with_rope(
            document.document().tree().root_node(),
            document.document().rope(),
        )
    })?;
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);
    let inferred_types = match inference_state.infer_module(&module) {
        Ok(inferred_types) => inferred_types,
        Err(error) => return Ok(render_expression_error_kind(&error).to_owned()),
    };

    Ok(render_expression_types(
        &mut inference_state,
        &lowering_context,
        &inferred_types,
    ))
}

fn bind_fixture_builtins(
    inference_state: &mut InferenceState,
    lowering_context: &mut LoweringContext,
) {
    let plus_symbol = lowering_context.intern("+");
    let minus_symbol = lowering_context.intern("-");
    let multiply_symbol = lowering_context.intern("*");
    let divide_symbol = lowering_context.intern("/");
    let power_symbol = lowering_context.intern("**");
    let and_symbol = lowering_context.intern("&&");
    let or_symbol = lowering_context.intern("||");
    let combine_symbol = lowering_context.intern("c");
    let list_symbol = lowering_context.intern("list");

    inference_state.bind_builtin(plus_symbol, BuiltinKind::Plus);
    inference_state.bind_builtin(minus_symbol, BuiltinKind::Minus);
    inference_state.bind_builtin(multiply_symbol, BuiltinKind::Multiply);
    inference_state.bind_builtin(divide_symbol, BuiltinKind::Divide);
    inference_state.bind_builtin(power_symbol, BuiltinKind::Power);
    inference_state.bind_builtin(and_symbol, BuiltinKind::And);
    inference_state.bind_builtin(or_symbol, BuiltinKind::Or);
    inference_state.bind_builtin(combine_symbol, BuiltinKind::Combine);
    inference_state.bind_builtin(list_symbol, BuiltinKind::List);
}
