// Keep this file focused on suite runners and helpers shared across multiple runners.
// Minimize fixture-runner logic so tests exercise real behavior as directly as possible.
// Helper logic that is only used by one runner should be inlined into that runner instead.
#[path = "fixture_renderers.rs"]
mod fixture_renderers;

use {
    analysis::{
        Analysis, AnalysisPhase, Document, DocumentId, HoverInfo, Interner, TextPosition,
        TextRange, check,
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
    fixtures::{Fixture, FixtureKind, FixtureOperation, FixtureRunFile, run_fixture_suite},
    ropey::Rope,
    std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    },
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
fn ide_hover() {
    run_fixture_suite("tests/ide/hover", run_ide_hover_fixture);
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

fn run_ide_hover_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::MultiFile(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };

    let mut analysis_state = Analysis::new(PathBuf::new());
    let mut hover_requests = BTreeMap::new();
    for document in &case.initial_generation.documents {
        if is_hover_fixture_path(&document.path) {
            hover_requests.insert(document.path.clone(), document.contents.clone());
            continue;
        }

        analysis_state
            .add_document_from_source(document.path.clone(), &document.contents)
            .map_err(|error| format!("failed to add `{}`: {error:?}", document.path.display()))?;
    }

    let mut outputs = vec![render_ide_hover_snapshot(
        &mut analysis_state,
        &hover_requests,
    )?];
    for generation in &case.generations {
        for entry in &generation.entries {
            apply_ide_hover_operation(&mut analysis_state, &mut hover_requests, &entry.operation)?;
        }

        outputs.push(render_ide_hover_snapshot(
            &mut analysis_state,
            &hover_requests,
        )?);
    }

    Ok(outputs)
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

fn render_ide_hover_snapshot(
    analysis_state: &mut Analysis,
    hover_requests: &BTreeMap<PathBuf, String>,
) -> Result<Vec<FixtureRunFile>, String> {
    let mut outputs = analysis_state
        .package_document_ids()
        .into_iter()
        .map(|document_id| {
            let path = analysis_state
                .path_for_document_id(document_id)
                .ok_or_else(|| "missing path for document".to_owned())?;
            Ok(FixtureRunFile {
                path: path.to_path_buf(),
                output: String::new(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    for (path, contents) in hover_requests {
        let request = parse_hover_request(contents)?;
        let hover = analysis_state.hover(&request.path, request.position);
        outputs.push(FixtureRunFile {
            path: path.clone(),
            output: render_hover_output(hover),
        });
    }

    outputs.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(outputs)
}

fn apply_ide_hover_operation(
    analysis_state: &mut Analysis,
    hover_requests: &mut BTreeMap<PathBuf, String>,
    operation: &FixtureOperation,
) -> Result<(), String> {
    match operation {
        FixtureOperation::CreateDocument { path, contents } => {
            if is_hover_fixture_path(path) {
                hover_requests.insert(path.clone(), contents.clone());
                return Ok(());
            }

            analysis_state
                .add_document_from_source(path.clone(), contents)
                .map_err(|error| format!("failed to add `{}`: {error:?}", path.display()))?;
            Ok(())
        }
        FixtureOperation::EditDocument {
            path,
            range,
            replacement_text,
        } => {
            if is_hover_fixture_path(path) {
                let contents = hover_requests.get_mut(path).ok_or_else(|| {
                    format!("missing hover fixture document `{}`", path.display())
                })?;
                edit_text(contents, hover_text_range(*range), replacement_text, path)?;
                return Ok(());
            }

            analysis_state
                .edit_document(path, |document, parser| {
                    document.edit_range(parser, hover_text_range(*range), replacement_text)
                })
                .map_err(|error| format!("failed to edit `{}`: {error:?}", path.display()))
        }
        FixtureOperation::MoveDocument {
            source_path,
            destination_path,
        } => {
            if is_hover_fixture_path(source_path) || is_hover_fixture_path(destination_path) {
                let contents = hover_requests.remove(source_path).ok_or_else(|| {
                    format!("missing hover fixture document `{}`", source_path.display())
                })?;
                hover_requests.insert(destination_path.clone(), contents);
                return Ok(());
            }

            let source = analysis_state
                .document(source_path)
                .ok_or_else(|| format!("missing source document `{}`", source_path.display()))?
                .rope()
                .to_string();
            analysis_state
                .add_document_from_source(destination_path.clone(), &source)
                .map_err(|error| {
                    format!("failed to move `{}`: {error:?}", destination_path.display())
                })?;
            analysis_state
                .delete_document(source_path)
                .map_err(|error| format!("failed to delete `{}`: {error:?}", source_path.display()))
        }
        FixtureOperation::DeleteDocument { path } => {
            if is_hover_fixture_path(path) {
                hover_requests.remove(path).ok_or_else(|| {
                    format!("missing hover fixture document `{}`", path.display())
                })?;
                return Ok(());
            }

            analysis_state
                .delete_document(path)
                .map_err(|error| format!("failed to delete `{}`: {error:?}", path.display()))
        }
    }
}

fn is_hover_fixture_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "hover")
}

fn hover_text_range(range: fixtures::FixtureRange) -> TextRange {
    TextRange {
        start: TextPosition {
            line_index: range.start.line_number - 1,
            character_index: range.start.column_number - 1,
        },
        end: TextPosition {
            line_index: range.end.line_number - 1,
            character_index: range.end.column_number - 1,
        },
    }
}

fn edit_text(
    contents: &mut String,
    range: TextRange,
    replacement_text: &str,
    path: &Path,
) -> Result<(), String> {
    let mut rope = Rope::from_str(contents);
    let start_character = analysis::text::line_character_to_character_index(&rope, range.start)
        .ok_or_else(|| format!("invalid edit range start in `{}`", path.display()))?;
    let end_character = analysis::text::line_character_to_character_index(&rope, range.end)
        .ok_or_else(|| format!("invalid edit range end in `{}`", path.display()))?;

    if start_character > end_character {
        return Err(format!("invalid edit range in `{}`", path.display()));
    }

    rope.remove(start_character..end_character);
    rope.insert(start_character, replacement_text);
    *contents = rope.to_string();
    Ok(())
}

fn parse_hover_request(contents: &str) -> Result<HoverRequest, String> {
    let request = contents.trim();
    let (path_and_line, column_number) = request
        .rsplit_once(':')
        .ok_or_else(|| "hover request must use `path:line:column`".to_owned())?;
    let (path, line_number) = path_and_line
        .rsplit_once(':')
        .ok_or_else(|| "hover request must use `path:line:column`".to_owned())?;
    let line_number = parse_hover_request_number(line_number, "line")?;
    let column_number = parse_hover_request_number(column_number, "column")?;

    Ok(HoverRequest {
        path: PathBuf::from(path),
        position: TextPosition {
            line_index: line_number - 1,
            character_index: column_number - 1,
        },
    })
}

fn parse_hover_request_number(text: &str, label: &str) -> Result<usize, String> {
    let value = text
        .parse::<usize>()
        .map_err(|error| format!("invalid hover request {label} `{text}`: {error}"))?;
    if value == 0 {
        return Err(format!("hover request {label} values are 1-based"));
    }
    Ok(value)
}

fn render_hover_output(hover: Option<HoverInfo>) -> String {
    let Some(hover) = hover else {
        return "no hover".to_owned();
    };

    let mut lines = vec![format!(
        "range: {}:{}-{}:{}",
        hover.range.start.line_index + 1,
        hover.range.start.character_index + 1,
        hover.range.end.line_index + 1,
        hover.range.end.character_index + 1
    )];

    for section in hover.sections {
        lines.push(String::new());
        lines.push(format!("[{}]", section.phase.title()));
        lines.push(section.value);
    }

    lines.join("\n")
}

struct HoverRequest {
    path: PathBuf,
    position: TextPosition,
}
