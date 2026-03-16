use {
    indoc::indoc,
    tree_sitter::{Point, Range},
    typing::{
        interner::Symbol,
        lower::{ExpressionKind, LoweringContext},
        new_parser, parse,
        types::{
            Annotation, Atomic, AttachedAnnotation, CoreType, FunctionType, InferenceVariableId,
            RecordField, SurfaceType, TypeScheme,
        },
    },
};

fn lower(source: &str) -> typing::Module {
    let mut parser = new_parser();
    let tree = parse(&mut parser, source, None);
    let mut lowering_context = LoweringContext::new();
    lowering_context.lower_tree(&tree, source)
}

fn lower_context(source: &str) -> LoweringContext {
    let mut parser = new_parser();
    let tree = parse(&mut parser, source, None);
    let mut lowering_context = LoweringContext::new();
    let _module = lowering_context.lower_tree(&tree, source);
    lowering_context
}

fn test_range() -> Range {
    Range {
        start_byte: 0,
        end_byte: 5,
        start_point: Point { row: 0, column: 0 },
        end_point: Point { row: 0, column: 5 },
    }
}

#[test]
fn lowering_context_interns_repeated_names() {
    let mut lowering_context = LoweringContext::new();

    let first_symbol = lowering_context.intern("value");
    let second_symbol = lowering_context.intern("value");

    assert_eq!(first_symbol, second_symbol);
    assert_eq!(lowering_context.resolve(first_symbol), Some("value"));
}

#[test]
fn lowering_context_assigns_unique_expression_ids() {
    let mut lowering_context = LoweringContext::new();
    let range = test_range();

    let first_expression = lowering_context.expression(range, ExpressionKind::Null);
    let second_expression = lowering_context.expression(range, ExpressionKind::Null);

    assert_ne!(first_expression.id, second_expression.id);
}

#[test]
fn lowered_symbol_expression_preserves_range_and_symbol() {
    let mut lowering_context = LoweringContext::new();
    let range = test_range();
    let symbol = lowering_context.intern("identity");

    let expression = lowering_context.expression(range, ExpressionKind::Symbol(symbol));

    assert_eq!(expression.range, range);
    assert_eq!(expression.annotation, None);
    assert_eq!(expression.kind, ExpressionKind::Symbol(symbol));
    assert_eq!(lowering_context.resolve(symbol), Some("identity"));
}

#[test]
fn lowered_assignment_uses_interned_target_symbol() {
    let mut lowering_context = LoweringContext::new();
    let range = test_range();
    let target = lowering_context.intern("result");
    let value = lowering_context.expression(range, ExpressionKind::Integer("1L".to_owned()));

    let assignment = lowering_context.expression(
        range,
        ExpressionKind::Assign {
            target,
            annotation: None,
            value: Box::new(value),
        },
    );

    match assignment.kind {
        ExpressionKind::Assign {
            target: assigned_target,
            annotation: None,
            ..
        } => {
            assert_eq!(assigned_target, target);
            assert_eq!(lowering_context.resolve(assigned_target), Some("result"));
        }
        other_kind => panic!("expected assignment, got {other_kind:?}"),
    }
}

#[test]
fn symbol_reexports_remain_public() {
    let mut interner = typing::Interner::new();
    let symbol: Symbol = interner.intern("public_name");

    assert_eq!(interner.resolve(symbol), Some("public_name"));
}

#[test]
fn lowering_assignment_snippet_interns_the_assigned_name() {
    let module = lower(indoc! {r#"
        value <- 1L
    "#});

    let expression = module
        .expressions
        .first()
        .expect("assignment expression should be lowered");

    match &expression.kind {
        ExpressionKind::Assign { target, .. } => {
            let mut lowering_context = LoweringContext::new();
            let expected_symbol = lowering_context.intern("value");
            assert_eq!(*target, expected_symbol);
        }
        other_kind => panic!("expected assignment, got {other_kind:?}"),
    }
}

#[test]
fn lowering_function_snippet_interns_function_parameters() {
    let lowering_context = lower_context(indoc! {r#"
        identity <- function(x) x
    "#});

    assert!(lowering_context.interner().contains("identity"));
    assert!(lowering_context.interner().contains("x"));
}

#[test]
fn lowering_call_snippet_interns_callee_and_named_argument() {
    let lowering_context = lower_context(indoc! {r#"
        compute(value = 1L)
    "#});

    assert!(lowering_context.interner().contains("compute"));
    assert!(lowering_context.interner().contains("value"));
}

#[test]
fn lowering_two_argument_call_snippet_interns_callee_only_once() {
    let lowering_context = lower_context(indoc! {r#"
        identity(1L, 2L)
    "#});

    assert!(lowering_context.interner().contains("identity"));
}

#[test]
fn lowering_plus_operator_snippet_interns_the_builtin_symbol() {
    let lowering_context = lower_context(indoc! {r#"
        "foo" + 4
    "#});

    assert!(lowering_context.interner().contains("+"));
}

#[test]
fn lowering_unsupported_snippet_produces_an_unsupported_expression() {
    let module = lower(indoc! {r#"
        if (flag) value else other
    "#});

    let expression = module
        .expressions
        .first()
        .expect("unsupported expression should be lowered");

    assert_eq!(expression.annotation, None);
    assert!(matches!(expression.kind, ExpressionKind::Unsupported));
}

#[test]
fn trailing_assignment_annotation_attaches_to_the_assignment_expression_and_binding() {
    let module = lower("value <- 1L #: integer\n");

    let expression = module
        .expressions
        .first()
        .expect("assignment expression should be lowered");

    assert_eq!(module.annotations.len(), 1);
    assert_eq!(
        expression.range.start_point.row,
        module.annotations[0].range.start_point.row
    );
    assert_eq!(
        expression.range.end_point.row,
        module.annotations[0].range.end_point.row
    );

    let expected_annotation =
        AttachedAnnotation::expression(Annotation::new(SurfaceType::Scalar(Atomic::Integer)));
    assert_eq!(expression.annotation, Some(expected_annotation));

    match &expression.kind {
        ExpressionKind::Assign {
            target: _,
            annotation,
            value,
        } => {
            assert_eq!(
                *annotation,
                Some(AttachedAnnotation::binding_and_expression(Annotation::new(
                    SurfaceType::Scalar(Atomic::Integer),
                )))
            );
            assert_eq!(value.annotation, None);
        }
        other_kind => panic!("expected assignment, got {other_kind:?}"),
    }
}

#[test]
fn monomorphic_type_scheme_has_no_quantified_variables() {
    let type_scheme = TypeScheme::monomorphic(CoreType::Scalar(Atomic::Integer));

    assert!(type_scheme.quantified_variables.is_empty());
    assert_eq!(type_scheme.body, CoreType::Scalar(Atomic::Integer));
}

#[test]
fn core_type_can_store_an_inference_variable() {
    let core_type = CoreType::Variable(InferenceVariableId(7));

    assert_eq!(core_type, CoreType::Variable(InferenceVariableId(7)));
}

#[test]
fn surface_function_type_preserves_named_and_positional_parameters() {
    let mut interner = typing::Interner::new();
    let named_parameter = interner.intern("count");

    let function_type = SurfaceType::Function(FunctionType::new(
        vec![SurfaceType::Scalar(Atomic::Character)],
        vec![RecordField::new(
            named_parameter,
            SurfaceType::Scalar(Atomic::Integer),
        )],
        SurfaceType::Scalar(Atomic::Logical),
    ));

    match function_type {
        SurfaceType::Function(function_type) => {
            assert_eq!(
                function_type.parameters,
                vec![SurfaceType::Scalar(Atomic::Character)]
            );
            assert_eq!(function_type.named_parameters.len(), 1);
            assert_eq!(function_type.named_parameters[0].name, named_parameter);
            assert_eq!(
                function_type.named_parameters[0].value,
                SurfaceType::Scalar(Atomic::Integer)
            );
            assert_eq!(
                *function_type.return_type,
                SurfaceType::Scalar(Atomic::Logical)
            );
        }
        other_type => panic!("expected function type, got {other_type:?}"),
    }
}

#[test]
fn core_record_type_preserves_interned_field_names() {
    let mut interner = typing::Interner::new();
    let field_name = interner.intern("name");

    let record_type = CoreType::Record(vec![RecordField::new(
        field_name,
        CoreType::Scalar(Atomic::Character),
    )]);

    match record_type {
        CoreType::Record(fields) => {
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].name, field_name);
            assert_eq!(interner.resolve(fields[0].name), Some("name"));
            assert_eq!(fields[0].value, CoreType::Scalar(Atomic::Character));
        }
        other_type => panic!("expected record type, got {other_type:?}"),
    }
}
