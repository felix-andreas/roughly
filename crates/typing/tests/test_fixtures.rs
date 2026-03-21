use {
    std::{collections::BTreeSet, fs, path::Path},
    typing::{
        AnalysisState, Interner,
        annotations::{
            parse_expanded_block_surface_type, parse_type_syntax_item, render_type_syntax_item,
        },
        infer::{BuiltinKind, InferenceError, InferenceState},
        lower::LoweringContext,
        new_parser, parse, render_surface_type,
        types::{Atomic, CoreType, InferenceVariableId},
    },
};

#[test]
fn diagnostics() {
    let mut parser = new_parser();
    let mut analysis_state = AnalysisState::new();

    run_fixture_suite("tests/diagnostics", "diagnostics", |code| {
        typing::check_source(code, &mut parser, &mut analysis_state).render(code)
    });
}

#[test]
fn inference() {
    run_fixture_suite(
        "tests/inference",
        "inference",
        render_inference_fixture_result,
    );
}

#[test]
fn annotations() {
    run_fixture_suite(
        "tests/annotations",
        "annotations",
        render_annotation_fixture_result,
    );
}

#[derive(Debug)]
struct TestGroup {
    name: String,
    cases: Vec<TestCase>,
}

#[derive(Debug)]
struct TestCase {
    name: String,
    code: String,
    expected: String,
}

fn run_fixture_suite<F>(directory_path: &str, kind: &str, render: F)
where
    F: FnMut(&str) -> String,
{
    let fixture_paths = collect_fixture_paths(directory_path, kind);
    let mut groups = Vec::new();

    for fixture_path in fixture_paths {
        let fixture_text = fs::read_to_string(&fixture_path).unwrap_or_else(|error| {
            panic!(
                "failed to read {kind} fixture file `{}`: {error}",
                fixture_path.display()
            )
        });
        groups.extend(parse_test_file(&fixture_text, kind));
    }

    run_test_groups(&groups, kind, render);
}

fn collect_fixture_paths(directory_path: &str, kind: &str) -> Vec<std::path::PathBuf> {
    let mut fixture_paths = Vec::new();
    collect_fixture_paths_recursively(Path::new(directory_path), &mut fixture_paths, kind);
    fixture_paths.sort();
    fixture_paths
}

fn collect_fixture_paths_recursively(
    directory_path: &Path,
    fixture_paths: &mut Vec<std::path::PathBuf>,
    kind: &str,
) {
    let entries = fs::read_dir(directory_path).unwrap_or_else(|error| {
        panic!(
            "failed to read {kind} fixture directory `{}`: {error}",
            directory_path.display()
        )
    });

    let mut entry_paths = entries
        .map(|entry| {
            entry.unwrap_or_else(|error| {
                panic!(
                    "failed to read entry in {kind} fixture directory `{}`: {error}",
                    directory_path.display()
                )
            })
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entry_paths.sort();

    for entry_path in entry_paths {
        if entry_path.is_dir() {
            collect_fixture_paths_recursively(&entry_path, fixture_paths, kind);
            continue;
        }

        if entry_path
            .extension()
            .is_some_and(|extension| extension == "test")
        {
            fixture_paths.push(entry_path);
        }
    }
}

fn parse_test_file(text: &str, kind: &str) -> Vec<TestGroup> {
    text.split("#====")
        .filter_map(|block| {
            if block.trim().is_empty() {
                return None;
            }

            let (name, cases) = block.split_once('\n').unwrap_or_else(|| {
                panic!("each test group must have a name line followed by content")
            });

            Some(TestGroup {
                name: name.trim().to_owned(),
                cases: cases
                    .split("#----")
                    .filter_map(|case| {
                        if case.trim().is_empty() {
                            return None;
                        }

                        let (name, body) = case.split_once('\n').unwrap_or_else(|| {
                            panic!("each test case must have a name line followed by content")
                        });

                        let (code, expected_block) =
                            body.split_once("#++++\n").unwrap_or_else(|| {
                                panic!(
                                    "each test case must include a `#++++` separator before the expected {kind} result"
                                )
                            });

                        Some(TestCase {
                            name: name.trim().to_owned(),
                            code: code.to_owned(),
                            expected: expected_block.trim_end().to_owned(),
                        })
                    })
                    .collect(),
            })
        })
        .collect()
}

fn run_test_groups<F>(groups: &[TestGroup], kind: &str, mut render: F)
where
    F: FnMut(&str) -> String,
{
    let maybe_filter = std::env::var("TYPING_FILTER").ok();
    let mut snapshot_names = BTreeSet::new();
    let mut failures = Vec::new();
    let mut executed_fixture_count = 0;

    for group in groups {
        for case in &group.cases {
            let snapshot_name = format!("{}__{}", group.name, case.name);

            assert!(
                snapshot_names.insert(snapshot_name.clone()),
                "duplicate typing {kind} test snapshot name `{snapshot_name}`"
            );

            if maybe_filter
                .as_ref()
                .is_some_and(|filter| !snapshot_name.contains(filter))
            {
                continue;
            }

            executed_fixture_count += 1;

            let rendered = render(&case.code);
            let rendered_trimmed = rendered.trim_end();
            if rendered_trimmed != case.expected {
                failures.push(format!(
                    "\u{1b}[1mfixture `{snapshot_name}` failed\u{1b}[0m\n\u{1b}[1minput:\u{1b}[0m\n{}\n\u{1b}[1mexpected:\u{1b}[0m\n{}\n\u{1b}[1mactual:\u{1b}[0m\n{}",
                    case.code.trim_end(),
                    case.expected,
                    rendered_trimmed
                ));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} {kind} test(s) failed:\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }

    eprintln!("{} {kind} fixture(s) passed", executed_fixture_count);
}

fn render_inference_fixture_result(source: &str) -> String {
    let mut parser = new_parser();
    let tree = parse(&mut parser, source, None);

    let mut lowering_context = LoweringContext::new();
    let module = lowering_context.lower_tree(&tree, source);

    let mut inference_state = InferenceState::new();
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

    match inference_state.infer_module(&module) {
        Ok(inferred_types) => {
            render_inferred_types(&mut inference_state, &lowering_context, &inferred_types)
        }
        Err(error) => render_inference_error_kind(&error).to_owned(),
    }
}

fn render_annotation_fixture_result(source: &str) -> String {
    let mut interner = Interner::new();
    let trimmed_source = source.trim();

    if trimmed_source.is_empty() {
        return String::new();
    }

    if let Some(expected_parse_error) = trimmed_source.strip_prefix("error:") {
        let normalized_source = expected_parse_error
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");

        if normalized_source.is_empty() {
            return "fixture error: annotation fixture parse-error case must include source after `error:`"
                .to_owned();
        }

        return match parse_type_syntax_item(&normalized_source, &mut interner) {
            Ok(item) => format!(
                "fixture error: expected parse error\nsource:\n{normalized_source}\nparsed as: {}",
                render_type_syntax_item(&item, &interner)
            ),
            Err(error) => format!("{error:?}"),
        };
    }

    if trimmed_source.lines().any(|line| {
        let trimmed_line = line.trim();
        trimmed_line.starts_with("@param ")
            || trimmed_line.starts_with("@return ")
            || trimmed_line.starts_with("@returns ")
    }) {
        return match parse_expanded_block_surface_type(trimmed_source, &mut interner) {
            Ok(surface_type) => render_surface_type(&surface_type, &interner),
            Err(error) => {
                format!("parse error: {error:?}\nsource:\n{trimmed_source}")
            }
        };
    }

    let normalized_source = trimmed_source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    match parse_type_syntax_item(&normalized_source, &mut interner) {
        Ok(item) => render_type_syntax_item(&item, &interner),
        Err(error) => format!("parse error: {error:?}\nsource:\n{trimmed_source}"),
    }
}

fn render_inferred_types(
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

fn render_inference_error_kind(error: &InferenceError) -> &'static str {
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

struct SimpleTypeRenderer<'a> {
    interner: &'a Interner,
    variable_names: std::collections::BTreeMap<InferenceVariableId, String>,
    next_variable_index: usize,
}

impl<'a> SimpleTypeRenderer<'a> {
    fn new(interner: &'a Interner) -> Self {
        Self {
            interner,
            variable_names: std::collections::BTreeMap::new(),
            next_variable_index: 0,
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
                        format!("{name}: {}", self.render(&parameter.value))
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
            CoreType::Variable(variable) => self.variable_name(*variable).to_owned(),
        }
    }

    fn variable_name(&mut self, variable: InferenceVariableId) -> &str {
        if !self.variable_names.contains_key(&variable) {
            let name = format!("type{}", self.next_variable_index + 1);
            self.next_variable_index += 1;
            self.variable_names.insert(variable, name);
        }

        self.variable_names
            .get(&variable)
            .map(String::as_str)
            .unwrap_or("type")
    }
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
