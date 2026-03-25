use {
    tree_sitter::{Point, Range},
    typing::{hir::ExpressionKind, lower::LoweringContext},
};

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

    assert_ne!(first_expression, second_expression);
}
