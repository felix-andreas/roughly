use {
    tree_sitter::{Point, Range},
    typing::{
        interner::Interner,
        infer::{InferenceEntry, InferenceError, InferenceState},
        lower::{Argument, Expression, ExpressionId, ExpressionKind, Module, Parameter},
        types::{Atomic, CoreType, FunctionType, InferenceVariableId, RecordField},
    },
};

fn test_range() -> Range {
    Range {
        start_byte: 0,
        end_byte: 1,
        start_point: Point { row: 0, column: 0 },
        end_point: Point { row: 0, column: 1 },
    }
}

fn expression(id: u32, kind: ExpressionKind) -> Expression {
    Expression::new(ExpressionId(id), test_range(), kind)
}

#[test]
fn fresh_variables_start_unbound() {
    let mut inference_state = InferenceState::new();

    let variable = inference_state.fresh_variable();

    assert_eq!(inference_state.entry(variable), Some(&InferenceEntry::Unbound));
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
    assert_eq!(inference_state.entry(left), Some(&InferenceEntry::Redirect(right)));
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

#[test]
fn inferring_an_integer_literal_returns_integer_scalar() {
    let mut inference_state = InferenceState::new();

    let inferred_type = inference_state
        .infer_expression(&expression(0, ExpressionKind::Integer("1L".to_owned())))
        .expect("integer literal should infer");

    assert_eq!(inferred_type, CoreType::Scalar(Atomic::Integer));
}

#[test]
fn inferring_a_known_symbol_uses_the_environment() {
    let mut inference_state = InferenceState::new();
    let mut interner = Interner::new();
    let name = interner.intern("value");
    inference_state.bind_name(name, CoreType::Scalar(Atomic::Double), test_range());

    let inferred_type = inference_state
        .infer_expression(&expression(0, ExpressionKind::Symbol(name)))
        .expect("known symbol should infer");

    assert_eq!(inferred_type, CoreType::Scalar(Atomic::Double));
}

#[test]
fn inferring_an_unknown_symbol_reports_an_error() {
    let mut inference_state = InferenceState::new();
    let mut interner = Interner::new();
    let name = interner.intern("missing");

    let result = inference_state.infer_expression(&expression(0, ExpressionKind::Symbol(name)));

    assert_eq!(
        result,
        Err(InferenceError::UnknownName {
            symbol: name,
            range: test_range(),
            expression_id: ExpressionId(0),
        })
    );
}

#[test]
fn inferring_an_assignment_binds_the_name() {
    let mut inference_state = InferenceState::new();
    let mut interner = Interner::new();
    let target = interner.intern("answer");

    let inferred_type = inference_state
        .infer_expression(&expression(
            0,
            ExpressionKind::Assign {
                target,
                value: Box::new(expression(1, ExpressionKind::Integer("1L".to_owned()))),
            },
        ))
        .expect("assignment should infer");

    assert_eq!(inferred_type, CoreType::Scalar(Atomic::Integer));
    assert_eq!(
        inference_state.lookup_name(target),
        Some(&typing::infer::Binding {
            core_type: CoreType::Scalar(Atomic::Integer),
            range: test_range(),
        })
    );
}

#[test]
fn inferring_a_function_produces_a_function_type() {
    let mut inference_state = InferenceState::new();
    let mut interner = Interner::new();
    let parameter_name = interner.intern("x");

    let inferred_type = inference_state
        .infer_expression(&expression(
            0,
            ExpressionKind::Function {
                parameters: vec![Parameter {
                    symbol: parameter_name,
                    range: test_range(),
                }],
                body: Box::new(expression(1, ExpressionKind::Symbol(parameter_name))),
            },
        ))
        .expect("function should infer");

    match inferred_type {
        CoreType::Function(function_type) => {
            assert_eq!(function_type.parameters.len(), 1);
            assert!(matches!(
                function_type.parameters[0],
                CoreType::Variable(InferenceVariableId(_))
            ));
            assert_eq!(*function_type.return_type, function_type.parameters[0].clone());
        }
        other_type => panic!("expected function type, got {other_type:?}"),
    }
}

#[test]
fn inferring_a_call_checks_argument_types() {
    let mut inference_state = InferenceState::new();
    let mut interner = Interner::new();
    let function_name = interner.intern("identity");
    let function_type = CoreType::Function(FunctionType::new(
        vec![CoreType::Scalar(Atomic::Integer)],
        Vec::new(),
        CoreType::Scalar(Atomic::Integer),
    ));
    inference_state.bind_name(function_name, function_type, test_range());

    let inferred_type = inference_state
        .infer_expression(&expression(
            0,
            ExpressionKind::Call {
                callee: Box::new(expression(1, ExpressionKind::Symbol(function_name))),
                arguments: vec![Argument {
                    expression: expression(2, ExpressionKind::Integer("1L".to_owned())),
                    name: None,
                }],
            },
        ))
        .expect("call should infer");

    assert_eq!(inferred_type, CoreType::Scalar(Atomic::Integer));
}

#[test]
fn inferring_a_two_argument_call_reports_the_actual_arity() {
    let mut inference_state = InferenceState::new();
    let mut interner = Interner::new();
    let function_name = interner.intern("identity");
    let function_type = CoreType::Function(FunctionType::new(
        vec![CoreType::Scalar(Atomic::Integer)],
        Vec::new(),
        CoreType::Scalar(Atomic::Integer),
    ));
    inference_state.bind_name(function_name, function_type, test_range());

    let result = inference_state.infer_expression(&expression(
        0,
        ExpressionKind::Call {
            callee: Box::new(expression(1, ExpressionKind::Symbol(function_name))),
            arguments: vec![
                Argument {
                    expression: expression(2, ExpressionKind::Integer("1L".to_owned())),
                    name: None,
                },
                Argument {
                    expression: expression(3, ExpressionKind::Integer("2L".to_owned())),
                    name: None,
                },
            ],
        },
    ));

    assert_eq!(
        result,
        Err(InferenceError::FunctionArityMismatch {
            expected: 1,
            actual: 2,
            range: Some(test_range()),
            expression_id: Some(ExpressionId(0)),
        })
    );
}

#[test]
fn inferring_a_module_processes_expressions_in_order() {
    let mut inference_state = InferenceState::new();
    let mut interner = Interner::new();
    let name = interner.intern("value");
    let module = Module::new(vec![
        expression(
            0,
            ExpressionKind::Assign {
                target: name,
                value: Box::new(expression(1, ExpressionKind::Integer("1L".to_owned()))),
            },
        ),
        expression(2, ExpressionKind::Symbol(name)),
    ]);

    let inferred_types = inference_state
        .infer_module(&module)
        .expect("module should infer");

    assert_eq!(
        inferred_types,
        vec![
            CoreType::Scalar(Atomic::Integer),
            CoreType::Scalar(Atomic::Integer),
        ]
    );
}

#[test]
fn unknown_inference_variables_are_reported() {
    let mut inference_state = InferenceState::new();

    let result = inference_state.resolve(CoreType::Variable(InferenceVariableId(99)));

    assert_eq!(
        result,
        Err(InferenceError::UnknownInferenceVariable(InferenceVariableId(
            99
        )))
    );
}
