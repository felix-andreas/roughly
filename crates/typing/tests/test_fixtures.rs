// Keep this file focused on suite runners and helpers shared across multiple runners.
// Helper logic that is only used by one runner should be inlined into that runner instead.
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
        typecheck::inference_state_with_builtins,
    },
};

const FIXTURE_PACKAGE_PATH: &str = "/fixture_package";
const FIXTURE_MAIN_DOCUMENT_PATH: &str = "/fixture_package/main.R";

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

fn package_for_fixture(fixture: &Fixture) -> Result<typing::Package, String> {
    let input_files = match &fixture.kind {
        FixtureKind::Simple(case) => vec![FixtureInputFile {
            path: PathBuf::from(FIXTURE_MAIN_DOCUMENT_PATH),
            contents: case.input.clone(),
        }],
        FixtureKind::MultiFile(case) => case.input_files.clone(),
        _ => return Err("unsupported fixture".to_owned()),
    };
    let mut workspace = typing::Workspace::new().map_err(|_| "workspace".to_owned())?;
    workspace
        .insert_package(PathBuf::from(FIXTURE_PACKAGE_PATH))
        .map_err(|_| "workspace".to_owned())?;

    for input_file in &input_files {
        workspace
            .insert_package_document(
                Path::new(FIXTURE_PACKAGE_PATH),
                input_file.path.clone(),
                &input_file.contents,
            )
            .map_err(|_| "workspace".to_owned())?;
    }

    workspace
        .package(Path::new(FIXTURE_PACKAGE_PATH))
        .cloned()
        .ok_or_else(|| "workspace".to_owned())
}

fn document_for_fixture(fixture: &Fixture) -> Result<typing::Document, String> {
    let FixtureKind::Simple(_) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let package = package_for_fixture(fixture)?;
    package
        .document(Path::new(FIXTURE_MAIN_DOCUMENT_PATH))
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
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
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

    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: lines.join("\n"),
        }],
    }])
}

fn run_diagnostics_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let package = package_for_fixture(fixture)?;
    let mut analysis_state = AnalysisState::new();
    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: typing::check(&package, &mut analysis_state).render(&case.input),
        }],
    }])
}

fn run_environment_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
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

    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: lines.join("\n"),
        }],
    }])
}

fn run_expressions_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
    let inferred_types = match inference_state.infer_module(&module) {
        Ok(inferred_types) => inferred_types,
        Err(error) => {
            return Ok(vec![FixtureOutput {
                name: fixture.name.clone(),
                files: vec![FixtureRunFile {
                    path: PathBuf::new(),
                    output: render_expression_error_kind(&error).to_owned(),
                }],
            }]);
        }
    };

    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: render_expression_types(
                &mut inference_state,
                &lowering_context,
                &inferred_types,
            ),
        }],
    }])
}

fn run_generalization_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
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

    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: lines.join("\n"),
        }],
    }])
}

fn run_instantiation_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
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

    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: lines.join("\n"),
        }],
    }])
}

fn run_interfaces_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);

    if inference_state.infer_module(&module).is_err() {
        return Ok(vec![FixtureOutput {
            name: fixture.name.clone(),
            files: vec![FixtureRunFile {
                path: PathBuf::new(),
                output: "error: inference".to_owned(),
            }],
        }]);
    }

    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: render_interface_snapshot(&module, &inference_state, &lowering_context),
        }],
    }])
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
        return Ok(vec![FixtureOutput {
            name: fixture.name.clone(),
            files: vec![FixtureRunFile {
                path: PathBuf::new(),
                output: render_diagnostics(&case.input, &diagnostics),
            }],
        }]);
    }

    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: module.render(&interner),
        }],
    }])
}

fn run_naming_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    struct NamingOutputFile {
        fixture_output_path: PathBuf,
        package_path: PathBuf,
        source: String,
    }

    let output_files = match &fixture.kind {
        FixtureKind::Simple(case) => vec![NamingOutputFile {
            fixture_output_path: PathBuf::new(),
            package_path: PathBuf::from(FIXTURE_MAIN_DOCUMENT_PATH),
            source: case.input.clone(),
        }],
        FixtureKind::MultiFile(case) => case
            .input_files
            .iter()
            .map(|input_file| NamingOutputFile {
                fixture_output_path: input_file.path.clone(),
                package_path: input_file.path.clone(),
                source: input_file.contents.clone(),
            })
            .collect(),
        _ => return Err("unsupported fixture".to_owned()),
    };
    let package = package_for_fixture(fixture)?;
    let mut analysis_state = AnalysisState::new();
    let package_result = run_lowering_and_naming(&package, &mut analysis_state);

    let mut files = Vec::with_capacity(output_files.len());
    for output_file in output_files {
        let output = if package_result.diagnostics.is_empty() {
            let module = package_result
                .modules
                .get(&output_file.package_path)
                .ok_or_else(|| "missing module".to_owned())?;
            render_named_hir(
                &module.arena,
                &module.definitions,
                &module.expressions,
                &package_result.naming,
                analysis_state.interner(),
            )
        } else {
            render_diagnostics(
                &output_file.source,
                &package_result
                    .diagnostics
                    .iter()
                    .find_map(|(diagnostic_path, diagnostics_for_path)| {
                        (diagnostic_path == &output_file.package_path)
                            .then(|| diagnostics_for_path.clone())
                    })
                    .unwrap_or_default(),
            )
        };
        files.push(FixtureRunFile {
            path: output_file.fixture_output_path,
            output,
        });
    }

    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files,
    }])
}

fn run_substitution_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
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

    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: lines.join("\n"),
        }],
    }])
}

fn run_type_syntax_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };

    let mut interner = Interner::new();
    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: match parse_type_syntax(&case.input, &mut interner) {
                Ok(item) => render_type_syntax(&item, &interner),
                Err(error) => format!("{error:?}"),
            },
        }],
    }])
}

fn run_unification_fixture(fixture: &Fixture) -> Result<Vec<FixtureOutput>, String> {
    let FixtureKind::Simple(_case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let document = document_for_fixture(fixture)?;
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
    let inferred_types = match inference_state.infer_module(&module) {
        Ok(inferred_types) => inferred_types,
        Err(error) => {
            return Ok(vec![FixtureOutput {
                name: fixture.name.clone(),
                files: vec![FixtureRunFile {
                    path: PathBuf::new(),
                    output: render_expression_error_kind(&error).to_owned(),
                }],
            }]);
        }
    };

    Ok(vec![FixtureOutput {
        name: fixture.name.clone(),
        files: vec![FixtureRunFile {
            path: PathBuf::new(),
            output: render_expression_types(
                &mut inference_state,
                &lowering_context,
                &inferred_types,
            ),
        }],
    }])
}
