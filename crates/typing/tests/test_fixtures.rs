#[path = "support.rs"]
mod support;

use {
    std::{collections::BTreeSet, fs, path::Path},
    support::{check_source, new_parser, parse_source},
    typing::{
        AnalysisState, Interner,
        hir::{DefinitionItem, DefinitionKind, ExpressionId, ExpressionKind, HirArena},
        lower::LoweringContext,
        naming::{BindingId, NamingContext, NamingResult},
        type_syntax::{parse_type_syntax, render_surface_type, render_type_syntax},
        typecheck::{BuiltinKind, InferenceError, InferenceState},
        types::{Atomic, CoreType, InferenceVariableId, TypeScheme},
    },
};

#[test]
fn lowering() {
    run_fixture_suite("tests/lowering", "lowering", render_lowering_fixture_result);
}

#[test]
fn naming() {
    run_fixture_suite("tests/naming", "naming", render_naming_fixture_result);
}

#[test]
fn diagnostics() {
    let mut parser = new_parser();
    let mut analysis_state = AnalysisState::new();

    run_fixture_suite("tests/diagnostics", "diagnostics", |code| {
        check_source(code, &mut parser, &mut analysis_state).render(code)
    });
}

#[test]
fn expressions() {
    run_fixture_suite(
        "tests/expressions",
        "expressions",
        render_expression_fixture_result,
    );
}

#[test]
fn unification() {
    run_fixture_suite(
        "tests/unification",
        "unification",
        render_unification_fixture_result,
    );
}

#[test]
fn generalization() {
    run_fixture_suite(
        "tests/generalization",
        "generalization",
        render_generalization_fixture_result,
    );
}

#[test]
fn instantiation() {
    run_fixture_suite(
        "tests/instantiation",
        "instantiation",
        render_instantiation_fixture_result,
    );
}

#[test]
fn bindings() {
    run_fixture_suite("tests/bindings", "bindings", render_bindings_fixture_result);
}

#[test]
fn substitution() {
    run_fixture_suite(
        "tests/substitution",
        "substitution",
        render_substitution_fixture_result,
    );
}

#[test]
fn environment() {
    run_fixture_suite(
        "tests/environment",
        "environment",
        render_environment_fixture_result,
    );
}

#[test]
fn interfaces() {
    run_fixture_suite(
        "tests/interfaces",
        "interfaces",
        render_interfaces_fixture_result,
    );
}

#[test]
fn type_syntax() {
    run_fixture_suite(
        "tests/type_syntax",
        "type_syntax",
        render_type_syntax_fixture_result,
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

fn render_lowering_fixture_result(source: &str) -> String {
    let mut parser = new_parser();
    let tree = parse_source(&mut parser, source);

    let mut lowering_context = LoweringContext::new();
    let lowering_result = lowering_context.lower_tree_with_diagnostics(&tree, source);

    if !lowering_result.diagnostics.is_empty() {
        return render_diagnostics(source, &lowering_result.diagnostics);
    }

    lowering_result.module.render(lowering_context.interner())
}

fn render_naming_fixture_result(source: &str) -> String {
    let mut parser = new_parser();
    let tree = parse_source(&mut parser, source);

    let mut lowering_context = LoweringContext::new();
    let lowering_result = lowering_context.lower_tree_with_diagnostics(&tree, source);
    if !lowering_result.diagnostics.is_empty() {
        return render_diagnostics(source, &lowering_result.diagnostics);
    }

    let module = lowering_result.module;
    let naming_result =
        NamingContext::new(&module.arena, lowering_context.interner()).resolve_module(&module);
    if !naming_result.diagnostics.is_empty() {
        return render_diagnostics(source, &naming_result.diagnostics);
    }

    render_named_module(&module, &naming_result, lowering_context.interner())
}

fn render_expression_fixture_result(source: &str) -> String {
    match infer_fixture_source(source) {
        Ok((lowering_context, _module, mut inference_state, inferred_types)) => {
            render_expression_types(&mut inference_state, &lowering_context, &inferred_types)
        }
        Err(error) => render_expression_error_kind(&error).to_owned(),
    }
}

fn render_diagnostics(source: &str, diagnostics: &[typing::Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "No diagnostics.\n".to_owned();
    }

    let mut rendered = String::new();

    for (index, diagnostic) in diagnostics.iter().enumerate() {
        if index > 0 {
            rendered.push('\n');
        }
        rendered.push_str(&diagnostic.render(source));
    }

    rendered
}

fn render_unification_fixture_result(source: &str) -> String {
    render_expression_fixture_result(source)
}

fn render_generalization_fixture_result(source: &str) -> String {
    match render_progressive_fixture_result(source, ProgressiveRenderMode::BindingsOnly) {
        Ok(rendered) => rendered,
        Err(error) => render_expression_error_kind(&error).to_owned(),
    }
}

fn render_bindings_fixture_result(source: &str) -> String {
    render_generalization_fixture_result(source)
}

fn render_substitution_fixture_result(source: &str) -> String {
    render_instantiation_fixture_result(source)
}

fn render_environment_fixture_result(source: &str) -> String {
    render_instantiation_fixture_result(source)
}

fn render_interfaces_fixture_result(source: &str) -> String {
    match infer_fixture_source(source) {
        Ok((lowering_context, module, inference_state, _inferred_types)) => {
            render_interface_snapshot(&module, &inference_state, &lowering_context)
        }
        Err(error) => render_expression_error_kind(&error).to_owned(),
    }
}

fn render_instantiation_fixture_result(source: &str) -> String {
    match render_progressive_fixture_result(source, ProgressiveRenderMode::BindingsAndResults) {
        Ok(rendered) => rendered,
        Err(error) => render_expression_error_kind(&error).to_owned(),
    }
}

fn infer_fixture_source(
    source: &str,
) -> Result<
    (
        LoweringContext,
        typing::Module,
        InferenceState,
        Vec<CoreType>,
    ),
    InferenceError,
> {
    let mut parser = new_parser();
    let tree = parse_source(&mut parser, source);

    let mut lowering_context = LoweringContext::new();
    let module = lowering_context.lower_tree(&tree, source);

    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);
    let inferred_types = inference_state.infer_module(&module)?;

    Ok((lowering_context, module, inference_state, inferred_types))
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

fn render_type_syntax_fixture_result(source: &str) -> String {
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

        return match parse_type_syntax(&normalized_source, &mut interner) {
            Ok(item) => format!(
                "fixture error: expected parse error\nsource:\n{normalized_source}\nparsed as: {}",
                render_type_syntax(&item, &interner)
            ),
            Err(error) => format!("{error:?}"),
        };
    }

    match parse_type_syntax(trimmed_source, &mut interner) {
        Ok(item) => render_type_syntax(&item, &interner),
        Err(error) => format!("parse error: {error:?}\nsource:\n{trimmed_source}"),
    }
}

fn render_expression_types(
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgressiveRenderMode {
    BindingsOnly,
    BindingsAndResults,
}

fn render_progressive_fixture_result(
    source: &str,
    mode: ProgressiveRenderMode,
) -> Result<String, InferenceError> {
    let mut parser = new_parser();
    let tree = parse_source(&mut parser, source);

    let mut lowering_context = LoweringContext::new();
    let module = lowering_context.lower_tree(&tree, source);

    let mut inference_state = InferenceState::new();
    bind_fixture_builtins(&mut inference_state, &mut lowering_context);

    let mut lines = Vec::new();

    for expression_id in &module.expressions {
        let expression = module.arena.get(*expression_id);
        let inferred_type = inference_state.infer_expression(expression, &module.arena)?;

        match &expression.kind {
            ExpressionKind::Assign { target, .. } => {
                let name = lowering_context
                    .interner()
                    .resolve(*target)
                    .unwrap_or("<unknown>");
                let binding = inference_state.lookup_name(*target).unwrap_or_else(|| {
                    panic!("binding `{name}` should be present after inference")
                });
                let mut renderer = SimpleTypeRenderer::new(lowering_context.interner());
                lines.push(format!(
                    "{name}: {}",
                    renderer.render_type_scheme(&binding.type_scheme)
                ));
            }
            _ if matches!(mode, ProgressiveRenderMode::BindingsAndResults) => {
                let resolved_type = inference_state.resolve(inferred_type)?;
                let mut renderer = SimpleTypeRenderer::new(lowering_context.interner());
                lines.push(renderer.render(&resolved_type));
            }
            _ => {}
        }
    }

    Ok(lines.join("\n"))
}

fn render_expression_error_kind(error: &InferenceError) -> &'static str {
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

fn render_named_module(
    module: &typing::Module,
    naming_result: &NamingResult,
    interner: &Interner,
) -> String {
    let mut lines = Vec::new();

    for definition in &module.definitions {
        render_named_definition(definition, interner, 0, &mut lines);
    }
    for expression_id in &module.expressions {
        render_named_expression(
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
                .map(|parameter| interner
                    .resolve(*parameter)
                    .unwrap_or("<unknown>")
                    .to_owned())
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
    arena: &HirArena,
    expression_id: ExpressionId,
    naming_result: &NamingResult,
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
                .get(&expression_id)
                .map(|binding_id| binding_label(*binding_id))
                .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}Symbol({name}@{binding})"));
        }
        ExpressionKind::Block { expressions, .. } => {
            lines.push(format!("{prefix}Block"));
            for nested_expression in expressions {
                render_named_expression(
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
                .get(&expression_id)
                .map(|binding_id| binding_label(*binding_id))
                .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}Assign({name}@{binding})"));
            render_named_expression(arena, *value, naming_result, interner, indent + 1, lines);
        }
        ExpressionKind::Function { parameters, body } => {
            let rendered_parameters = parameters
                .iter()
                .map(|parameter| {
                    let name = interner.resolve(parameter.symbol).unwrap_or("<unknown>");
                    let binding = find_binding_by_symbol_and_range(
                        naming_result,
                        parameter.symbol,
                        parameter.range,
                    )
                    .map(|binding_id| binding_label(binding_id))
                    .unwrap_or_else(|| "?".to_owned());
                    format!("{name}@{binding}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("{prefix}Function({rendered_parameters})"));
            render_named_expression(arena, *body, naming_result, interner, indent + 1, lines);
        }
        ExpressionKind::If {
            condition,
            consequence,
            alternative,
        } => {
            lines.push(format!("{prefix}If"));
            render_named_expression(
                arena,
                *condition,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
            render_named_expression(
                arena,
                *consequence,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
            if let Some(alternative) = alternative {
                render_named_expression(
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
            let binding =
                find_binding_by_symbol_and_range(naming_result, *variable, expression.range)
                    .map(|binding_id| binding_label(binding_id))
                    .unwrap_or_else(|| "?".to_owned());
            lines.push(format!("{prefix}For({name}@{binding})"));
            render_named_expression(arena, *sequence, naming_result, interner, indent + 1, lines);
            render_named_expression(arena, *body, naming_result, interner, indent + 1, lines);
        }
        ExpressionKind::While { condition, body } => {
            lines.push(format!("{prefix}While"));
            render_named_expression(
                arena,
                *condition,
                naming_result,
                interner,
                indent + 1,
                lines,
            );
            render_named_expression(arena, *body, naming_result, interner, indent + 1, lines);
        }
        ExpressionKind::Repeat { body } => {
            lines.push(format!("{prefix}Repeat"));
            render_named_expression(arena, *body, naming_result, interner, indent + 1, lines);
        }
        ExpressionKind::UnaryMinus { value } => {
            lines.push(format!("{prefix}UnaryMinus"));
            render_named_expression(arena, *value, naming_result, interner, indent + 1, lines);
        }
        ExpressionKind::Call { callee, arguments } => {
            lines.push(format!("{prefix}Call"));
            render_named_expression(arena, *callee, naming_result, interner, indent + 1, lines);
            for argument in arguments {
                let argument_prefix = "  ".repeat(indent + 1);
                if let Some(name) = argument.name {
                    let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                    lines.push(format!("{argument_prefix}Argument({rendered_name})"));
                } else {
                    lines.push(format!("{argument_prefix}Argument"));
                }
                render_named_expression(
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
            render_named_expression(arena, *value, naming_result, interner, indent + 1, lines);
            for argument in arguments {
                let argument_prefix = "  ".repeat(indent + 1);
                if let Some(name) = argument.name {
                    let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                    lines.push(format!("{argument_prefix}Argument({rendered_name})"));
                } else {
                    lines.push(format!("{argument_prefix}Argument"));
                }
                render_named_expression(
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
            render_named_expression(arena, *value, naming_result, interner, indent + 1, lines);
            for argument in arguments {
                let argument_prefix = "  ".repeat(indent + 1);
                if let Some(name) = argument.name {
                    let rendered_name = interner.resolve(name).unwrap_or("<unknown>");
                    lines.push(format!("{argument_prefix}Argument({rendered_name})"));
                } else {
                    lines.push(format!("{argument_prefix}Argument"));
                }
                render_named_expression(
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
            render_named_expression(arena, *value, naming_result, interner, indent + 1, lines);
        }
        ExpressionKind::Unsupported => lines.push(format!("{prefix}Unsupported")),
    }
}

fn binding_label(binding_id: BindingId) -> String {
    format!("b{}", binding_id.0)
}

fn find_binding_by_symbol_and_range(
    naming_result: &NamingResult,
    symbol: typing::Symbol,
    range: tree_sitter::Range,
) -> Option<BindingId> {
    naming_result
        .bindings
        .values()
        .find(|binding| binding.symbol == symbol && binding.range == range)
        .map(|binding| binding.id)
}

fn render_interface_snapshot(
    module: &typing::Module,
    inference_state: &InferenceState,
    lowering_context: &LoweringContext,
) -> String {
    let mut exported_entries = Vec::<(usize, typing::Symbol, String)>::new();

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
