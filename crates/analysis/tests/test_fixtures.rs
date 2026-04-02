// Keep this file focused on suite runners and helpers shared across multiple runners.
// Minimize fixture-runner logic so tests exercise real behavior as directly as possible.
// Helper logic that is only used by one runner should be inlined into that runner instead.
#[path = "fixture_renderers.rs"]
mod fixture_renderers;

use {
    analysis::{
        Analysis, AnalysisPhase, Document, DocumentId, Interner, check,
        hir::ExpressionKind,
        lower::{self, LoweringContext},
        naming::resolve_document_locally,
        run_lowering, run_naming,
        tree::new_parser,
        type_syntax::{parse_type_syntax, render_type_syntax},
        typecheck::inference_state_with_builtins,
    },
    fixture_renderers::{
        render_core_type, render_expression_error_kind, render_expression_types,
        render_interface_snapshot, render_locally_named_hir, render_named_hir, render_type_scheme,
    },
    fixtures::{Fixture, FixtureKind, FixtureRunFile, run_fixture_suite},
    std::path::PathBuf,
};

#[test]
fn bindings() {
    run_fixture_suite("tests/bindings", run_bindings_fixture);
}

#[test]
fn diagnostics() {
    run_fixture_suite("tests/diagnostics", run_diagnostics_fixture);
}

#[test]
fn environment() {
    run_fixture_suite("tests/environment", run_environment_fixture);
}

#[test]
fn expressions() {
    run_fixture_suite("tests/expressions", run_expressions_fixture);
}

#[test]
fn generalization() {
    run_fixture_suite("tests/generalization", run_generalization_fixture);
}

#[test]
fn instantiation() {
    run_fixture_suite("tests/instantiation", run_instantiation_fixture);
}

#[test]
fn interfaces() {
    run_fixture_suite("tests/interfaces", run_interfaces_fixture);
}

#[test]
fn lowering() {
    run_fixture_suite("tests/lowering", run_lowering_fixture);
}

#[test]
fn naming_global() {
    run_fixture_suite("tests/naming/global", run_naming_global_fixture);
}

#[test]
fn naming_local() {
    run_fixture_suite("tests/naming/local", run_naming_local_fixture);
}

#[test]
fn substitution() {
    run_fixture_suite("tests/substitution", run_substitution_fixture);
}

#[test]
fn type_syntax() {
    run_fixture_suite("tests/type_syntax", run_type_syntax_fixture);
}

#[test]
fn unification() {
    run_fixture_suite("tests/unification", run_unification_fixture);
}

fn run_bindings_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
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

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: lines.join("\n"),
    }]])
}

fn run_diagnostics_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
    let mut analysis_state = Analysis::new(PathBuf::new());
    analysis_state.add_document(PathBuf::from("R/main.R"), document);
    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: check(&mut analysis_state).render(&case.input),
    }]])
}

fn run_environment_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
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

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: lines.join("\n"),
    }]])
}

fn run_expressions_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
    let inferred_types = match inference_state.infer_module(&module) {
        Ok(inferred_types) => inferred_types,
        Err(error) => {
            return Ok(vec![vec![FixtureRunFile {
                path: PathBuf::new(),
                output: render_expression_error_kind(&error).to_owned(),
            }]]);
        }
    };

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: render_expression_types(&mut inference_state, &lowering_context, &inferred_types),
    }]])
}

fn run_generalization_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
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

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: lines.join("\n"),
    }]])
}

fn run_instantiation_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
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

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: lines.join("\n"),
    }]])
}

fn run_interfaces_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);

    if inference_state.infer_module(&module).is_err() {
        return Ok(vec![vec![FixtureRunFile {
            path: PathBuf::new(),
            output: "error: inference".to_owned(),
        }]]);
    }

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: render_interface_snapshot(&module, &inference_state, &lowering_context),
    }]])
}

fn run_lowering_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let diagnostics = lowering_context.take_diagnostics();
    let interner = lowering_context.interner().clone();

    if !diagnostics.is_empty() {
        return Ok(vec![vec![FixtureRunFile {
            path: PathBuf::new(),
            output: diagnostics
                .iter()
                .map(|diagnostic| diagnostic.render(&case.input))
                .collect::<Vec<_>>()
                .join("\n"),
        }]]);
    }

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: module.render(&interner),
    }]])
}

fn run_naming_local_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let lowering_diagnostics = lowering_context.take_diagnostics();
    let document_id = DocumentId(0);
    let local_naming_result = resolve_document_locally(document_id, &module);
    let rendered_hir = render_locally_named_hir(
        document_id,
        &module,
        &local_naming_result,
        lowering_context.interner(),
    );
    let rendered_diagnostics = if lowering_diagnostics.is_empty() {
        "No diagnostics.\n".to_owned()
    } else {
        lowering_diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(&case.input))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let output = [rendered_hir, rendered_diagnostics]
        .into_iter()
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output,
    }]])
}

fn run_naming_global_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::MultiFile(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let mut analysis_state = Analysis::new(PathBuf::new());
    for document in &case.initial_generation.documents {
        let parsed_document =
            Document::parse(&mut parser, &document.contents).expect("parse fixture document");
        analysis_state.add_document(document.path.clone(), parsed_document);
    }

    run_lowering(None, &mut analysis_state);
    run_naming(None, &mut analysis_state);

    let files = case
        .initial_generation
        .documents
        .iter()
        .map(|document| {
            let document_id = analysis_state
                .document_id_for_path(&document.path)
                .ok_or_else(|| "missing document id".to_owned())?;
            let module = analysis_state
                .module(document_id)
                .ok_or_else(|| "missing module".to_owned())?;
            let rendered_hir = render_named_hir(
                document_id,
                module,
                &analysis_state.naming.package,
                analysis_state.interner(),
            );
            let diagnostics = analysis_state
                .document_phase_diagnostics(
                    document_id,
                    &[AnalysisPhase::Lowering, AnalysisPhase::Naming],
                )
                .collect::<Vec<_>>();
            let rendered_diagnostics = if diagnostics.is_empty() {
                String::new()
            } else {
                diagnostics
                    .into_iter()
                    .map(|diagnostic| diagnostic.render(&document.contents))
                    .collect::<Vec<_>>()
                    .join("\n")
            };

            Ok(FixtureRunFile {
                path: document.path.clone(),
                output: [rendered_hir, rendered_diagnostics]
                    .into_iter()
                    .filter(|section| !section.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(vec![files])
}

fn run_substitution_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
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

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: lines.join("\n"),
    }]])
}

fn run_type_syntax_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };

    let mut interner = Interner::new();
    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: match parse_type_syntax(&case.input, &mut interner) {
            Ok(item) => render_type_syntax(&item, &interner),
            Err(error) => format!("{error:?}"),
        },
    }]])
}

fn run_unification_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
    let inferred_types = match inference_state.infer_module(&module) {
        Ok(inferred_types) => inferred_types,
        Err(error) => {
            return Ok(vec![vec![FixtureRunFile {
                path: PathBuf::new(),
                output: render_expression_error_kind(&error).to_owned(),
            }]]);
        }
    };

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: render_expression_types(&mut inference_state, &lowering_context, &inferred_types),
    }]])
}
