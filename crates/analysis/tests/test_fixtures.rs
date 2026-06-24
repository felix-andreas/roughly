// Keep this file focused on suite runners and helpers shared across multiple runners.
// Minimize fixture-runner logic so tests exercise real behavior as directly as possible.
// Helper logic that is only used by one runner should be inlined into that runner instead.
#[path = "fixture_renderers.rs"]
mod fixture_renderers;

use {
    analysis::{
        Analysis, CheckConfig, Document, DocumentChange, DocumentId, Interner, LintConfig,
        TextPosition, TextRange,
        hir::ExpressionKind,
        ide,
        lint::{self, NameStyle},
        lower::{self, LoweringContext},
        naming::{DocumentKind, resolve_document_locally},
        render_diagnostics, render_hover_markdown, render_type_scheme, resolve_package,
        text::line_character_to_character_index,
        tree::new_parser,
        type_syntax::{parse_type_syntax, render_type_syntax},
        typecheck::{TypeDefinitionEnvironment, inference_state_with_builtins},
    },
    fixture_renderers::{
        render_expression_error_kind, render_expression_types, render_interface_snapshot,
        render_locally_named_hir, render_named_hir,
    },
    fixtures::{Fixture, FixtureKind, FixtureOperation, FixtureRunFile, run_fixture_suite},
    std::{
        collections::{BTreeMap, HashMap},
        path::{Path, PathBuf},
    },
};

#[test]
fn diagnostics() {
    run_fixture_suite("tests/diagnostics", run_diagnostics_fixture);
}

#[test]
fn lint() {
    run_fixture_suite("tests/lint", run_lint_fixture);
}

#[test]
fn ide() {
    run_fixture_suite("tests/ide", run_ide_fixture);
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
fn type_syntax() {
    run_fixture_suite("tests/type_syntax", run_type_syntax_fixture);
}

#[test]
fn typecheck_bindings() {
    run_fixture_suite("tests/typecheck/bindings", run_bindings_fixture);
}

#[test]
fn typecheck_expressions() {
    run_fixture_suite("tests/typecheck/expressions", run_expressions_fixture);
}

#[test]
fn typecheck_interfaces() {
    run_fixture_suite("tests/typecheck/interfaces", run_interfaces_fixture);
}

#[test]
fn typecheck_project() {
    run_fixture_suite("tests/typecheck/project", run_project_fixture);
}

#[test]
fn typecheck_unification() {
    run_fixture_suite("tests/typecheck/unification", run_unification_fixture);
}

fn run_project_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::MultiFile(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut analysis_state = Analysis::new(
        PathBuf::new(),
        LintConfig::default(),
        CheckConfig {
            unused: false,
            typing: true,
        },
    );

    for entry in &case.initial_generation.entries {
        apply_project_operation(&mut analysis_state, &entry.operation)?;
    }
    let mut snapshots = vec![render_project_snapshot(&mut analysis_state)?];
    for generation in &case.generations {
        for entry in &generation.entries {
            apply_project_operation(&mut analysis_state, &entry.operation)?;
        }
        snapshots.push(render_project_snapshot(&mut analysis_state)?);
    }

    return Ok(snapshots);

    fn apply_project_operation(
        analysis_state: &mut Analysis,
        operation: &FixtureOperation,
    ) -> Result<(), String> {
        match operation {
            FixtureOperation::CreateDocument { path, contents } => analysis_state
                .add_document_from_source(path.clone(), contents)
                .map(|_| ())
                .map_err(|error| format!("failed to add `{}`: {error:?}", path.display())),
            FixtureOperation::EditDocument {
                path,
                range,
                replacement_text,
            } => analysis_state
                .edit_document(
                    path,
                    &[DocumentChange {
                        range: fixture_text_range(*range),
                        text: replacement_text.clone(),
                    }],
                )
                .map_err(|error| format!("failed to edit `{}`: {error:?}", path.display())),
            FixtureOperation::MoveDocument {
                source_path,
                destination_path,
            } => {
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
                    .map_err(|error| {
                        format!("failed to delete `{}`: {error:?}", source_path.display())
                    })
            }
            FixtureOperation::DeleteDocument { path } => analysis_state
                .delete_document(path)
                .map_err(|error| format!("failed to delete `{}`: {error:?}", path.display())),
            FixtureOperation::Action { action, .. } => {
                Err(format!("project fixtures do not support `{action}` actions"))
            }
        }
    }

    fn render_project_snapshot(
        analysis_state: &mut Analysis,
    ) -> Result<Vec<FixtureRunFile>, String> {
        analysis::run_full(analysis_state);
        let mut files = analysis_state
            .all_document_ids()
            .into_iter()
            .map(|document_id| {
                let path = analysis_state
                    .path_for_document_id(document_id)
                    .ok_or_else(|| "missing path for document".to_owned())?
                    .to_path_buf();
                let contents = analysis_state
                    .document_by_id(document_id)
                    .ok_or_else(|| "missing document".to_owned())?
                    .rope()
                    .to_string();
                Ok(FixtureRunFile {
                    output: render_diagnostics(
                        &contents,
                        &analysis_state.document_diagnostics(document_id),
                    ),
                    path,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    }

    fn fixture_text_range(range: fixtures::FixtureRange) -> TextRange {
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
    let type_definitions = TypeDefinitionEnvironment::from_module(&module);
    let mut lines = Vec::new();

    for expression_id in &module.expressions {
        let expression = module.arena.get(*expression_id);
        inference_state
            .infer_expression(expression, &module.arena, &type_definitions)
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
    let mut analysis_state = Analysis::new(
        PathBuf::new(),
        LintConfig::default(),
        CheckConfig {
            unused: false,
            typing: true,
        },
    );
    analysis_state.add_document(PathBuf::from("R/main.R"), document);
    analysis::run_full(&mut analysis_state);
    let document_id = analysis_state
        .document_id_for_path(Path::new("R/main.R"))
        .ok_or_else(|| "missing document id".to_owned())?;
    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: render_diagnostics(
            &case.input,
            &analysis_state.document_diagnostics(document_id),
        ),
    }]])
}

fn run_lint_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
    let diagnostics = lint::analyze(
        &document,
        LintConfig {
            naming_style: Some(NameStyle::Snake),
        },
    );

    Ok(vec![vec![FixtureRunFile {
        path: PathBuf::new(),
        output: render_diagnostics(&case.input, &diagnostics),
    }]])
}

fn run_expressions_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    run_expression_type_fixture(fixture)
}

fn run_expression_type_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
    let type_definitions = TypeDefinitionEnvironment::from_module(&module);
    let inferred_types = match inference_state.infer_module(&module, &type_definitions) {
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

fn run_interfaces_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
    let mut lowering_context = LoweringContext::new();
    let module = lower::lower(&document, &mut lowering_context);
    let mut inference_state = inference_state_with_builtins(&mut lowering_context);
    let type_definitions = TypeDefinitionEnvironment::from_module(&module);

    if inference_state
        .infer_module(&module, &type_definitions)
        .is_err()
    {
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

fn run_ide_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::MultiFile(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };

    let mut analysis_state = Analysis::new(
        PathBuf::new(),
        LintConfig::default(),
        CheckConfig {
            unused: false,
            typing: true,
        },
    );
    for entry in &case.initial_generation.entries {
        apply_ide_operation(&mut analysis_state, &entry.operation)?;
    }

    let mut outputs = vec![render_ide_snapshot(
        &mut analysis_state,
        &case.initial_generation.entries,
    )?];
    for generation in &case.generations {
        for entry in &generation.entries {
            apply_ide_operation(&mut analysis_state, &entry.operation)?;
        }

        outputs.push(render_ide_snapshot(
            &mut analysis_state,
            &generation.entries,
        )?);
    }

    return Ok(outputs);

    fn apply_ide_operation(
        analysis_state: &mut Analysis,
        operation: &FixtureOperation,
    ) -> Result<(), String> {
        match operation {
            FixtureOperation::CreateDocument { path, contents } => {
                analysis_state
                    .add_document_from_source(path.clone(), contents)
                    .map_err(|error| format!("failed to add `{}`: {error:?}", path.display()))?;
                Ok(())
            }
            FixtureOperation::EditDocument {
                path,
                range,
                replacement_text,
            } => analysis_state
                .edit_document(
                    path,
                    &[DocumentChange {
                        range: hover_text_range(*range),
                        text: replacement_text.clone(),
                    }],
                )
                .map_err(|error| format!("failed to edit `{}`: {error:?}", path.display())),
            FixtureOperation::MoveDocument {
                source_path,
                destination_path,
            } => {
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
                    .map_err(|error| {
                        format!("failed to delete `{}`: {error:?}", source_path.display())
                    })
            }
            FixtureOperation::DeleteDocument { path } => analysis_state
                .delete_document(path)
                .map_err(|error| format!("failed to delete `{}`: {error:?}", path.display())),
            FixtureOperation::Action { .. } => Ok(()),
        }
    }

    fn render_ide_snapshot(
        analysis_state: &mut Analysis,
        entries: &[fixtures::FixtureGenerationEntry],
    ) -> Result<Vec<FixtureRunFile>, String> {
        let mut outputs = analysis_state
            .package_document_ids()
            .into_iter()
            .map(|document_id| {
                let path = analysis_state
                    .path_for_document_id(document_id)
                    .ok_or_else(|| "missing path for document".to_owned())?;
                Ok((path.to_path_buf(), String::new()))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;

        for entry in entries {
            let FixtureOperation::Action {
                action,
                path,
                contents,
            } = &entry.operation
            else {
                continue;
            };

            let output = match action.as_str() {
                "hover" => {
                    let request = parse_position_request(contents, "hover")?;
                    let hover = ide::hover(analysis_state, &request.path, request.position);
                    match hover.as_ref() {
                        Some(hover) => render_hover_markdown(hover, false),
                        None => "no hover".to_owned(),
                    }
                }
                "rename" => {
                    let request = parse_rename_request(contents)?;
                    match ide::rename(
                        analysis_state,
                        &request.path,
                        request.position,
                        &request.new_name,
                    ) {
                        Some(rename_result) => {
                            render_rename_result(analysis_state, &rename_result)?
                        }
                        None => "no rename".to_owned(),
                    }
                }
                "completion" => {
                    let request = parse_position_request(contents, "completion")?;
                    match ide::completion(analysis_state, &request.path, request.position) {
                        Some(items) => render_completion_items(&items),
                        None => "no completions".to_owned(),
                    }
                }
                "goto_definition" => {
                    let request = parse_position_request(contents, "goto_definition")?;
                    match ide::definition(analysis_state, &request.path, request.position) {
                        Some(locations) => render_locations(&locations, &[]),
                        None => "no definition".to_owned(),
                    }
                }
                "references" => {
                    let request = parse_position_request(contents, "references")?;
                    match ide::references(analysis_state, &request.path, request.position, true) {
                        Some(locations) => {
                            let declarations =
                                ide::definition(analysis_state, &request.path, request.position)
                                    .unwrap_or_default();
                            render_locations(&locations, &declarations)
                        }
                        None => "no references".to_owned(),
                    }
                }
                "signature_help" => {
                    let request = parse_position_request(contents, "signature_help")?;
                    match ide::signature_help(analysis_state, &request.path, request.position) {
                        Some(help) => {
                            let active = match help.active_parameter {
                                Some(index) => index.to_string(),
                                None => "none".to_owned(),
                            };
                            format!("{}\nactive parameter: {active}", help.label)
                        }
                        None => "no signature help".to_owned(),
                    }
                }
                "inlay_hints" => {
                    let path = PathBuf::from(contents.trim());
                    let hints = ide::inlay_hints(analysis_state, &path);
                    if hints.is_empty() {
                        "no inlay hints".to_owned()
                    } else {
                        hints
                            .iter()
                            .map(|hint| {
                                format!(
                                    "{}:{}{}",
                                    hint.position.line_index + 1,
                                    hint.position.character_index + 1,
                                    hint.label
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    }
                }
                _ => {
                    panic!("IDE action `{action}` is not implemented yet")
                }
            };

            outputs.insert(path.clone(), output);
        }

        Ok(outputs
            .into_iter()
            .map(|(path, output)| FixtureRunFile { path, output })
            .collect())
    }

    struct PositionRequest {
        path: PathBuf,
        position: TextPosition,
    }

    struct RenameRequest {
        path: PathBuf,
        position: TextPosition,
        new_name: String,
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

    fn parse_position_request(contents: &str, label: &str) -> Result<PositionRequest, String> {
        fn parse_request_number(
            text: &str,
            request_label: &str,
            part_label: &str,
        ) -> Result<usize, String> {
            let value = text.parse::<usize>().map_err(|error| {
                format!("invalid {request_label} request {part_label} `{text}`: {error}")
            })?;
            if value == 0 {
                return Err(format!(
                    "{request_label} request {part_label} values are 1-based"
                ));
            }
            Ok(value)
        }

        let request = contents.trim();
        let (path_and_line, column_number) = request
            .rsplit_once(':')
            .ok_or_else(|| format!("{label} request must use `path:line:column`"))?;
        let (path, line_number) = path_and_line
            .rsplit_once(':')
            .ok_or_else(|| format!("{label} request must use `path:line:column`"))?;
        let line_number = parse_request_number(line_number, label, "line")?;
        let column_number = parse_request_number(column_number, label, "column")?;

        Ok(PositionRequest {
            path: PathBuf::from(path),
            position: TextPosition {
                line_index: line_number - 1,
                character_index: column_number - 1,
            },
        })
    }

    fn parse_rename_request(contents: &str) -> Result<RenameRequest, String> {
        let (location, new_name) = contents
            .trim()
            .split_once("->")
            .ok_or_else(|| "rename request must use `path:line:column -> new_name`".to_owned())?;
        let request = parse_position_request(location.trim(), "rename")?;
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err("rename request new_name cannot be empty".to_owned());
        }

        Ok(RenameRequest {
            path: request.path,
            position: request.position,
            new_name: new_name.to_owned(),
        })
    }

    fn render_locations(
        locations: &[analysis::Location],
        declarations: &[analysis::Location],
    ) -> String {
        locations
            .iter()
            .map(|location| {
                let marker = if declarations.contains(location) {
                    " [declaration]"
                } else {
                    ""
                };
                format!(
                    "{}:{}:{}..{}:{}{marker}",
                    location.path.display(),
                    location.range.start.line_index + 1,
                    location.range.start.character_index + 1,
                    location.range.end.line_index + 1,
                    location.range.end.character_index + 1,
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_completion_items(items: &[analysis::CompletionItem]) -> String {
        items
            .iter()
            .map(|item| {
                let source = match item.source {
                    analysis::CompletionItemSource::Keyword => "keyword",
                    analysis::CompletionItemSource::Local => "local",
                    analysis::CompletionItemSource::Global => "global",
                };
                let kind = match item.kind {
                    analysis::CompletionItemKind::Keyword => "keyword",
                    analysis::CompletionItemKind::Variable => "variable",
                    analysis::CompletionItemKind::Function => "function",
                };
                format!("{} [{source} {kind}]", item.label)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_rename_result(
        analysis_state: &Analysis,
        rename_result: &analysis::RenameResult,
    ) -> Result<String, String> {
        let mut before = Vec::new();
        let mut after = Vec::new();

        for (path, edits) in &rename_result.edits {
            let source = analysis_state
                .document(path)
                .ok_or_else(|| format!("missing rename document `{}`", path.display()))?
                .rope()
                .to_string();
            let renamed = apply_text_edits(&source, edits)?;
            before.push(render_named_file(path, &source));
            after.push(render_named_file(path, &renamed));
        }

        Ok(format!(
            "before:\n{}\n\nafter:\n{}",
            before.join("\n"),
            after.join("\n")
        ))
    }

    fn render_named_file(path: &Path, contents: &str) -> String {
        format!("{}:\n{}", path.display(), contents.trim_end())
    }

    fn apply_text_edits(source: &str, edits: &[analysis::RenameEdit]) -> Result<String, String> {
        let rope = ropey::Rope::from_str(source);
        let mut replacements = edits
            .iter()
            .map(|edit| {
                let start = line_character_to_character_index(&rope, edit.range.start)
                    .ok_or_else(|| "rename edit start is out of bounds".to_owned())?;
                let end = line_character_to_character_index(&rope, edit.range.end)
                    .ok_or_else(|| "rename edit end is out of bounds".to_owned())?;
                Ok((start, end, edit.replacement_text.clone()))
            })
            .collect::<Result<Vec<_>, String>>()?;
        replacements.sort_by(|left, right| right.0.cmp(&left.0));

        let mut rendered = source.to_owned();
        for (start, end, replacement_text) in replacements {
            rendered.replace_range(start..end, &replacement_text);
        }
        Ok(rendered)
    }
}

fn run_lowering_fixture(fixture: &Fixture) -> Result<Vec<Vec<FixtureRunFile>>, String> {
    let FixtureKind::Simple(case) = &fixture.kind else {
        return Err("unsupported fixture".to_owned());
    };
    let mut parser = new_parser().unwrap();
    let document = Document::parse(&mut parser, &case.input).expect("parse fixture");
    let mut analysis_state = Analysis::new(
        PathBuf::new(),
        LintConfig::default(),
        CheckConfig::default(),
    );
    let document_id = analysis_state.add_document(PathBuf::from("R/main.R"), document);
    analysis::lower(&mut analysis_state);
    let diagnostics = analysis_state.document_diagnostics(document_id);

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
        output: analysis_state
            .module(document_id)
            .expect("lowered module should exist")
            .render(analysis_state.interner()),
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
    let local_naming = resolve_document_locally(
        document_id,
        &module,
        lowering_context.interner(),
        DocumentKind::Package,
    );
    let rendered_hir = render_locally_named_hir(
        document_id,
        &module,
        &local_naming.naming,
        lowering_context.interner(),
    );
    let mut diagnostics = lowering_diagnostics;
    diagnostics.extend(local_naming.diagnostics);
    let rendered_diagnostics = if diagnostics.is_empty() {
        "No diagnostics.\n".to_owned()
    } else {
        diagnostics
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
    let mut analysis_state = Analysis::new(
        PathBuf::new(),
        LintConfig::default(),
        CheckConfig::default(),
    );
    for entry in &case.initial_generation.entries {
        let FixtureOperation::CreateDocument { path, contents } = &entry.operation else {
            continue;
        };
        let parsed_document =
            Document::parse(&mut parser, contents).expect("parse fixture document");
        analysis_state.add_document(path.clone(), parsed_document);
    }

    analysis::lower(&mut analysis_state);
    resolve_package(&mut analysis_state);

    let mut ordered_document_ids = analysis_state.package_document_ids();
    let mut non_package_document_ids = case
        .initial_generation
        .entries
        .iter()
        .filter_map(|entry| {
            let FixtureOperation::CreateDocument { path, .. } = &entry.operation else {
                return None;
            };
            analysis_state.document_id_for_path(path)
        })
        .filter(|document_id| !ordered_document_ids.contains(document_id))
        .collect::<Vec<_>>();
    non_package_document_ids.sort_by(|left_document_id, right_document_id| {
        let left_path = analysis_state
            .path_for_document_id(*left_document_id)
            .expect("document path should exist")
            .to_path_buf();
        let right_path = analysis_state
            .path_for_document_id(*right_document_id)
            .expect("document path should exist")
            .to_path_buf();
        left_path.cmp(&right_path)
    });
    ordered_document_ids.extend(non_package_document_ids);

    let mut next_display_binding_id = 0u32;
    let mut binding_display_labels = BTreeMap::new();
    let all_local_naming = ordered_document_ids
        .iter()
        .map(|document_id| {
            analysis_state
                .document_naming(*document_id)
                .cloned()
                .map(|local_naming| (*document_id, local_naming))
                .ok_or_else(|| "missing local naming".to_owned())
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    for document_id in &ordered_document_ids {
        let local_naming = all_local_naming
            .get(document_id)
            .ok_or_else(|| "missing local naming".to_owned())?;
        let mut binding_ids = local_naming.bindings.keys().copied().collect::<Vec<_>>();
        binding_ids.sort_by_key(|binding_id| binding_id.0);
        for binding_id in binding_ids {
            binding_display_labels.insert(
                (*document_id, binding_id),
                format!("b{next_display_binding_id}"),
            );
            next_display_binding_id += 1;
        }
    }

    let mut files = case
        .initial_generation
        .entries
        .iter()
        .filter_map(|entry| {
            let FixtureOperation::CreateDocument { path, contents } = &entry.operation else {
                return None;
            };
            Some((path, contents))
        })
        .map(|(path, contents)| {
            let document_id = analysis_state
                .document_id_for_path(path)
                .ok_or_else(|| "missing document id".to_owned())?;
            let module = analysis_state
                .module(document_id)
                .ok_or_else(|| "missing module".to_owned())?;
            let local_naming = analysis_state
                .document_naming(document_id)
                .ok_or_else(|| "missing local naming".to_owned())?;
            let rendered_hir = render_named_hir(
                &analysis_state,
                document_id,
                module,
                local_naming,
                &all_local_naming,
                analysis_state
                    .package_naming()
                    .ok_or_else(|| "missing package naming".to_owned())?,
                &binding_display_labels,
                analysis_state.interner(),
            );
            let diagnostics = analysis_state.document_diagnostics(document_id);
            let rendered_diagnostics = if diagnostics.is_empty() {
                String::new()
            } else {
                diagnostics
                    .into_iter()
                    .map(|diagnostic| diagnostic.render(contents))
                    .collect::<Vec<_>>()
                    .join("\n")
            };

            Ok(FixtureRunFile {
                path: path.clone(),
                output: [rendered_hir, rendered_diagnostics]
                    .into_iter()
                    .filter(|section| !section.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    files.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(vec![files])
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
    run_expression_type_fixture(fixture)
}
