use {
    std::collections::BTreeSet,
    tree_sitter::{Point, Range},
    typing::{
        infer::{InferenceEntry, InferenceError, InferenceState},
        interner::Interner,
        lower::LoweringContext,
        new_parser, parse,
        types::{Atomic, CoreType, FunctionType, InferenceVariableId, RecordField},
    },
};

#[test]
fn inference_cases() {
    const INFERENCE_TESTS: &str = include_str!("fixtures/inference.R.test");
    run_test_groups(&parse_test_file(INFERENCE_TESTS));
}

#[derive(Debug)]
struct TestGroup {
    name: &'static str,
    cases: Vec<TestCase>,
}

#[derive(Debug)]
struct TestCase {
    name: &'static str,
    code: &'static str,
    expected: &'static str,
}

fn parse_test_file(text: &'static str) -> Vec<TestGroup> {
    text.split("#====")
        .filter_map(|block| {
            if block.trim().is_empty() {
                return None;
            }

            let (name, cases) = block.split_once('\n').unwrap_or_else(|| {
                panic!("each test group must have a name line followed by content")
            });

            Some(TestGroup {
                name: name.trim(),
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
                                    "each test case must include a `#++++` separator before the expected inference result"
                                )
                            });

                        Some(TestCase {
                            name: name.trim(),
                            code,
                            expected: expected_block.trim_end(),
                        })
                    })
                    .collect(),
            })
        })
        .collect()
}

fn run_test_groups(groups: &[TestGroup]) {
    let maybe_filter = std::env::var("TYPING_FILTER").ok();
    let mut snapshot_names = BTreeSet::new();

    for group in groups {
        for case in &group.cases {
            let snapshot_name = format!("{}__{}", group.name, case.name);

            assert!(
                snapshot_names.insert(snapshot_name.clone()),
                "duplicate typing inference test snapshot name `{snapshot_name}`"
            );

            if maybe_filter
                .as_ref()
                .is_some_and(|filter| !snapshot_name.contains(filter))
            {
                continue;
            }

            let rendered = render_inference_result(case.code);
            assert_eq!(
                rendered.trim_end(),
                case.expected,
                "fixture `{snapshot_name}`"
            );
        }
    }
}

fn render_inference_result(source: &str) -> String {
    let mut parser = new_parser();
    let tree = parse(&mut parser, source, None);

    let mut lowering_context = LoweringContext::new();
    let module = lowering_context.lower_tree(&tree, source);

    let mut inference_state = InferenceState::new();
    let plus_symbol = lowering_context.intern("+");
    let combine_symbol = lowering_context.intern("c");

    inference_state.bind_name(plus_symbol, builtin_plus_type(), builtin_range());
    inference_state.bind_name(combine_symbol, builtin_combine_type(), builtin_range());

    match inference_state.infer_module(&module) {
        Ok(inferred_types) => {
            render_inferred_types(&mut inference_state, &lowering_context, &inferred_types)
        }
        Err(error) => render_inference_error_kind(&error).to_owned(),
    }
}

fn render_inferred_types(
    inference_state: &mut InferenceState,
    lowering_context: &LoweringContext,
    inferred_types: &[CoreType],
) -> String {
    let mut renderer = SimpleTypeRenderer::new(lowering_context.interner());
    let mut lines = Vec::with_capacity(inferred_types.len());

    for inferred_type in inferred_types {
        let resolved_type = inference_state
            .resolve(inferred_type.clone())
            .unwrap_or_else(|error| {
                panic!("inference result should resolve for rendering: {error:?}")
            });
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
        InferenceError::RecordFieldMismatch { .. } => "error: record field mismatch",
        InferenceError::FunctionArityMismatch { .. } => "error: function arity mismatch",
        InferenceError::NamedParameterMismatch { .. } => "error: named parameter mismatch",
    }
}

fn builtin_plus_type() -> CoreType {
    CoreType::Function(FunctionType::new(
        vec![CoreType::Unknown, CoreType::Unknown],
        Vec::new(),
        CoreType::Unknown,
    ))
}

fn builtin_combine_type() -> CoreType {
    CoreType::Function(FunctionType::new(
        vec![CoreType::Unknown],
        Vec::new(),
        CoreType::Unknown,
    ))
}

fn builtin_range() -> Range {
    Range {
        start_byte: 0,
        end_byte: 0,
        start_point: Point { row: 0, column: 0 },
        end_point: Point { row: 0, column: 0 },
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
            CoreType::Any => "any".to_owned(),
            CoreType::Unknown => "unknown".to_owned(),
            CoreType::Null => "null".to_owned(),
            CoreType::Scalar(atomic) => render_atomic(*atomic).to_owned(),
            CoreType::Vector(atomic) => format!("{}[]", render_atomic(*atomic)),
            CoreType::List(item_type) => format!("list[{}]", self.render(item_type)),
            CoreType::Record(fields) => {
                let rendered_fields = fields
                    .iter()
                    .map(|field| {
                        let name = self.interner.resolve(field.name).unwrap_or("<unknown>");
                        format!("{name}: {}", self.render(&field.value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("record{{{rendered_fields}}}")
            }
            CoreType::Tuple(items) => {
                let rendered_items = items
                    .iter()
                    .map(|item| self.render(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("tuple({rendered_items})")
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

#[test]
fn fresh_variables_start_unbound() {
    let mut inference_state = InferenceState::new();

    let variable = inference_state.fresh_variable();

    assert_eq!(
        inference_state.entry(variable),
        Some(&InferenceEntry::Unbound)
    );
}

#[test]
fn unifying_two_variables_creates_a_redirect() {
    let mut inference_state = InferenceState::new();
    let left = inference_state.fresh_variable();
    let right = inference_state.fresh_variable();

    let unified_type = inference_state
        .unify(CoreType::Variable(left), CoreType::Variable(right))
        .expect("variables should unify");

    assert_eq!(unified_type, CoreType::Variable(right));
    assert_eq!(
        inference_state.entry(left),
        Some(&InferenceEntry::Redirect(right))
    );
}

#[test]
fn resolving_a_redirected_variable_applies_path_compression() {
    let mut inference_state = InferenceState::new();
    let first = inference_state.fresh_variable();
    let second = inference_state.fresh_variable();
    let third = inference_state.fresh_variable();

    inference_state
        .unify(CoreType::Variable(first), CoreType::Variable(second))
        .expect("first and second should unify");
    inference_state
        .unify(CoreType::Variable(second), CoreType::Variable(third))
        .expect("second and third should unify");
    inference_state
        .unify(CoreType::Variable(third), CoreType::Scalar(Atomic::Integer))
        .expect("third should unify with integer");

    let resolved_type = inference_state
        .resolve(CoreType::Variable(first))
        .expect("first variable should resolve");

    assert_eq!(resolved_type, CoreType::Scalar(Atomic::Integer));
    assert_eq!(
        inference_state.entry(first),
        Some(&InferenceEntry::Bound(CoreType::Scalar(Atomic::Integer)))
    );
}

#[test]
fn occurs_check_rejects_recursive_types() {
    let mut inference_state = InferenceState::new();
    let variable = inference_state.fresh_variable();

    let result = inference_state.unify(
        CoreType::Variable(variable),
        CoreType::List(Box::new(CoreType::Variable(variable))),
    );

    assert_eq!(
        result,
        Err(InferenceError::OccursCheckFailed {
            variable,
            in_type: CoreType::List(Box::new(CoreType::Variable(variable))),
            range: None,
            expression_id: None,
        })
    );
}

#[test]
fn scalar_types_unify_only_when_equal() {
    let mut inference_state = InferenceState::new();

    let result = inference_state.unify(
        CoreType::Scalar(Atomic::Integer),
        CoreType::Scalar(Atomic::Double),
    );

    assert_eq!(
        result,
        Err(InferenceError::TypeMismatch {
            expected: CoreType::Scalar(Atomic::Integer),
            actual: CoreType::Scalar(Atomic::Double),
            range: None,
            expression_id: None,
        })
    );
}

#[test]
fn function_types_unify_structurally() {
    let mut inference_state = InferenceState::new();
    let left = CoreType::Function(FunctionType::new(
        vec![CoreType::Scalar(Atomic::Integer)],
        Vec::new(),
        CoreType::Scalar(Atomic::Logical),
    ));
    let right = CoreType::Function(FunctionType::new(
        vec![CoreType::Scalar(Atomic::Integer)],
        Vec::new(),
        CoreType::Scalar(Atomic::Logical),
    ));

    let unified_type = inference_state
        .unify(left, right)
        .expect("functions should unify");

    assert_eq!(
        unified_type,
        CoreType::Function(FunctionType::new(
            vec![CoreType::Scalar(Atomic::Integer)],
            Vec::new(),
            CoreType::Scalar(Atomic::Logical),
        ))
    );
}

#[test]
fn record_types_require_the_same_field_names() {
    let mut inference_state = InferenceState::new();
    let mut interner = Interner::new();
    let left_name = interner.intern("left");
    let right_name = interner.intern("right");

    let left = CoreType::Record(vec![RecordField::new(
        left_name,
        CoreType::Scalar(Atomic::Integer),
    )]);
    let right = CoreType::Record(vec![RecordField::new(
        right_name,
        CoreType::Scalar(Atomic::Integer),
    )]);

    let result = inference_state.unify(left, right);

    assert_eq!(
        result,
        Err(InferenceError::RecordFieldMismatch {
            expected_fields: vec![left_name],
            actual_fields: vec![right_name],
            range: None,
            expression_id: None,
        })
    );
}

#[test]
fn tuple_types_require_the_same_length() {
    let mut inference_state = InferenceState::new();

    let result = inference_state.unify(
        CoreType::Tuple(vec![CoreType::Scalar(Atomic::Integer)]),
        CoreType::Tuple(vec![
            CoreType::Scalar(Atomic::Integer),
            CoreType::Scalar(Atomic::Integer),
        ]),
    );

    assert_eq!(
        result,
        Err(InferenceError::TupleLengthMismatch {
            expected: 1,
            actual: 2,
            range: None,
            expression_id: None,
        })
    );
}

#[test]
fn named_function_parameters_require_the_same_names() {
    let mut inference_state = InferenceState::new();
    let mut interner = Interner::new();
    let left_name = interner.intern("left");
    let right_name = interner.intern("right");

    let left = CoreType::Function(FunctionType::new(
        Vec::new(),
        vec![RecordField::new(
            left_name,
            CoreType::Scalar(Atomic::Integer),
        )],
        CoreType::Scalar(Atomic::Logical),
    ));
    let right = CoreType::Function(FunctionType::new(
        Vec::new(),
        vec![RecordField::new(
            right_name,
            CoreType::Scalar(Atomic::Integer),
        )],
        CoreType::Scalar(Atomic::Logical),
    ));

    let result = inference_state.unify(left, right);

    assert_eq!(
        result,
        Err(InferenceError::NamedParameterMismatch {
            expected_parameters: vec![left_name],
            actual_parameters: vec![right_name],
            range: None,
            expression_id: None,
        })
    );
}
