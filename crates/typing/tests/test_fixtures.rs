#[path = "fixture_renderers.rs"]
mod fixture_renderers;

use {
    fixture_renderers::{
        render_core_type, render_diagnostics, render_expression_error_kind,
        render_expression_types, render_interface_snapshot, render_named_hir, render_type_scheme,
    },
    fixtures::{
        Fixture, FixtureInputFile, FixtureKind, FixtureOutput, FixtureRunFile, run_fixture_suite,
    },
    std::path::{Path, PathBuf},
    typing::{
        AnalysisState, Interner,
        hir::ExpressionKind,
        lower::{self, LoweringContext},
        run_lowering_and_naming,
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

fn workspace_for_fixture(
    fixture: &Fixture,
    use_package_documents: bool,
) -> Result<typing::Workspace, String> {
    const SINGLE_FILE_PHASE_DOCUMENT_PATH: &str = "/single_file_phase_fixture.R";
    const PACKAGE_PHASE_PATH: &str = "/package_phase_fixture";
    const PACKAGE_PHASE_DOCUMENT_PATH: &str = "/package_phase_fixture.R";

    let input_files = match &fixture.kind {
        FixtureKind::Simple(case) => vec![FixtureInputFile {
            path: PathBuf::from(if use_package_documents {
                PACKAGE_PHASE_DOCUMENT_PATH
            } else {
                SINGLE_FILE_PHASE_DOCUMENT_PATH
            }),
            contents: case.input.clone(),
        }],
        FixtureKind::MultiFile(case) => case.input_files.clone(),
        _ => return Err("unsupported fixture".to_owned()),
    };

    let mut workspace = typing::Workspace::new().map_err(|_| "workspace".to_owned())?;
    if use_package_documents {
        workspace
            .insert_package(PathBuf::from(PACKAGE_PHASE_PATH))
            .map_err(|_| "workspace".to_owned())?;
    }

    for input_file in &input_files {
        let result = if use_package_documents {
            workspace.insert_package_document(
                Path::new(PACKAGE_PHASE_PATH),
                input_file.path.clone(),
                &input_file.contents,
            )
        } else {
            workspace.insert_workspace_script(input_file.path.clone(), &input_file.contents)
        };
        result.map_err(|_| "workspace".to_owned())?;
    }

    Ok(workspace)
}

fn document_for_fixture(fixture: &Fixture) -> Result<typing::Document, String> {
    let workspace = workspace_for_fixture(fixture, false)?;
    workspace
        .document(Path::new("/single_file_phase_fixture.R"))
        .cloned()
        .ok_or_else(|| "workspace".to_owned())
}

fn package_for_fixture(fixture: &Fixture) -> Result<typing::Package, String> {
    let workspace = workspace_for_fixture(fixture, true)?;
    workspace
        .package(Path::new("/package_phase_fixture"))
        .cloned()
        .ok_or_else(|| "workspace".to_owned())
}

fn run_bindings_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
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

    Ok(single_snapshot_output(&fixture.name, lines.join("\n")))
}

fn run_diagnostics_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let package = package_for_fixture(fixture)?;
    let mut analysis_state = AnalysisState::new();
    Ok(single_snapshot_output(
        &fixture.name,
        typing::check(&package, &mut analysis_state).render(&case.input),
    ))
}

fn run_environment_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
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

    Ok(single_snapshot_output(&fixture.name, lines.join("\n")))
}

fn run_expressions_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);
    let inferred_types = match inference_state.infer_module(&module) {
        Ok(inferred_types) => inferred_types,
        Err(error) => {
            return Ok(single_snapshot_output(
                &fixture.name,
                render_expression_error_kind(&error).to_owned(),
            ));
        }
    };

    Ok(single_snapshot_output(
        &fixture.name,
        render_expression_types(&mut inference_state, &lowering_context, &inferred_types),
    ))
}

fn run_generalization_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
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

    Ok(single_snapshot_output(&fixture.name, lines.join("\n")))
}

fn run_instantiation_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
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

    Ok(single_snapshot_output(&fixture.name, lines.join("\n")))
}

fn run_interfaces_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);

    if inference_state.infer_module(&module).is_err() {
        return Ok(single_snapshot_output(
            &fixture.name,
            "error: inference".to_owned(),
        ));
    }

    Ok(single_snapshot_output(
        &fixture.name,
        render_interface_snapshot(&module, &inference_state, &lowering_context),
    ))
}

fn run_lowering_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let diagnostics = lowering_context.take_diagnostics();
    let interner = lowering_context.interner().clone();

    if !diagnostics.is_empty() {
        return Ok(single_snapshot_output(
            &fixture.name,
            render_diagnostics(&case.input, &diagnostics),
        ));
    }

    Ok(single_snapshot_output(
        &fixture.name,
        module.render(&interner),
    ))
}

fn run_naming_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let package = package_for_fixture(fixture)?;
    let mut analysis_state = AnalysisState::new();
    let package_result = run_lowering_and_naming(&package, &mut analysis_state);
    let interner = analysis_state.interner().clone();

    if !package_result.diagnostics.is_empty() {
        if let FixtureKind::Simple(case) = &fixture.kind {
            return Ok(single_snapshot_output(
                &fixture.name,
                render_diagnostics(&case.input, &package_result.diagnostics),
            ));
        }
        return Err("multi-file naming".to_owned());
    }

    match &fixture.kind {
        FixtureKind::Simple(_) => {
            let module = package_result
                .modules
                .values()
                .next()
                .ok_or_else(|| "missing module".to_owned())?;

            Ok(single_snapshot_output(
                &fixture.name,
                render_named_hir(
                    &module.arena,
                    &module.definitions,
                    &module.expressions,
                    &package_result.naming,
                    &interner,
                ),
            ))
        }
        FixtureKind::MultiFile(case) => {
            let mut rendered_outputs = Vec::with_capacity(case.expectations.len());
            for expectation in &case.expectations {
                let module = package_result
                    .modules
                    .get(&expectation.path)
                    .ok_or_else(|| "missing file".to_owned())?;
                rendered_outputs.push(FixtureRunFile {
                    path: expectation.path.clone(),
                    output: render_named_hir(
                        &module.arena,
                        &module.definitions,
                        &module.expressions,
                        &package_result.naming,
                        &interner,
                    ),
                });
            }

            Ok(files_snapshot_output(&fixture.name, rendered_outputs))
        }
        _ => Err("unsupported fixture".to_owned()),
    }
}

fn run_substitution_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
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

    Ok(single_snapshot_output(&fixture.name, lines.join("\n")))
}

fn run_type_syntax_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };

    let mut interner = Interner::new();
    Ok(single_snapshot_output(
        &fixture.name,
        match parse_type_syntax(&case.input, &mut interner) {
            Ok(item) => render_type_syntax(&item, &interner),
            Err(error) => format!("{error:?}"),
        },
    ))
}

fn run_unification_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);
    let inferred_types = match inference_state.infer_module(&module) {
        Ok(inferred_types) => inferred_types,
        Err(error) => {
            return Ok(single_snapshot_output(
                &fixture.name,
                render_expression_error_kind(&error).to_owned(),
            ));
        }
    };

    Ok(single_snapshot_output(
        &fixture.name,
        render_expression_types(&mut inference_state, &lowering_context, &inferred_types),
    ))
}

fn single_snapshot_output(name: &str, output: String) -> Vec<FixtureOutput> {
    vec![FixtureOutput {
        name: name.to_owned(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output,
        }],
    }]
}

fn files_snapshot_output(name: &str, files: Vec<FixtureRunFile>) -> Vec<FixtureOutput> {
    vec![FixtureOutput {
        name: name.to_owned(),
        files,
    }]
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
