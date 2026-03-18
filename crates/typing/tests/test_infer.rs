use typing::{
    infer::{InferenceEntry, InferenceError, InferenceState},
    interner::Interner,
    types::{Atomic, CoreType, FunctionType, RecordField},
};

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
