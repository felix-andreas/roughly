use {
    crate::{
        annotations::parse_annotation,
        interner::{Interner, Symbol},
        text,
        types::{Annotation, AttachedAnnotation},
    },
    ropey::Rope,
    tree_sitter::{Node, Range, Tree},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExpressionId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub expressions: Vec<Expression>,
    pub annotations: Vec<PendingAnnotation>,
}

impl Module {
    pub fn new(expressions: Vec<Expression>, annotations: Vec<PendingAnnotation>) -> Self {
        Self {
            expressions,
            annotations,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expression {
    pub id: ExpressionId,
    pub range: Range,
    pub annotation: Option<AttachedAnnotation>,
    pub kind: ExpressionKind,
}

impl Expression {
    pub fn new(
        id: ExpressionId,
        range: Range,
        annotation: Option<AttachedAnnotation>,
        kind: ExpressionKind,
    ) -> Self {
        Self {
            id,
            range,
            annotation,
            kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionKind {
    Null,
    Logical(bool),
    Integer(String),
    Double(String),
    Character(String),
    StringLiteralName(Symbol),
    Symbol(Symbol),
    Block {
        expressions: Vec<Expression>,
        has_trailing_semicolon: bool,
    },
    Assign {
        target: Symbol,
        annotation: Option<AttachedAnnotation>,
        value: Box<Expression>,
    },
    Function {
        parameters: Vec<Parameter>,
        body: Box<Expression>,
    },
    If {
        condition: Box<Expression>,
        consequence: Box<Expression>,
        alternative: Option<Box<Expression>>,
    },
    For {
        variable: Symbol,
        sequence: Box<Expression>,
        body: Box<Expression>,
    },
    While {
        condition: Box<Expression>,
        body: Box<Expression>,
    },
    Repeat {
        body: Box<Expression>,
    },
    UnaryMinus {
        value: Box<Expression>,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Argument>,
    },
    Subset {
        value: Box<Expression>,
        arguments: Vec<Argument>,
    },
    Subset2 {
        value: Box<Expression>,
        arguments: Vec<Argument>,
    },
    Dollar {
        value: Box<Expression>,
        name: Symbol,
    },
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub symbol: Symbol,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub expression: Expression,
    pub name: Option<Symbol>,
}

#[derive(Debug, Default)]
pub struct LoweringContext {
    next_expression_id: u32,
    interner: Interner,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingAnnotation {
    pub range: Range,
    pub annotation: Annotation,
}

impl LoweringContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh_expression_id(&mut self) -> ExpressionId {
        let expression_id = ExpressionId(self.next_expression_id);
        self.next_expression_id += 1;
        expression_id
    }

    pub fn intern(&mut self, text: &str) -> Symbol {
        self.interner.intern(text)
    }

    pub fn resolve(&self, symbol: Symbol) -> Option<&str> {
        self.interner.resolve(symbol)
    }

    pub fn interner(&self) -> &Interner {
        &self.interner
    }

    pub fn interner_mut(&mut self) -> &mut Interner {
        &mut self.interner
    }

    pub fn expression(&mut self, range: Range, kind: ExpressionKind) -> Expression {
        self.annotated_expression(range, None, kind)
    }

    pub fn annotated_expression(
        &mut self,
        range: Range,
        annotation: Option<AttachedAnnotation>,
        kind: ExpressionKind,
    ) -> Expression {
        Expression::new(self.fresh_expression_id(), range, annotation, kind)
    }

    pub fn lower_tree(&mut self, tree: &Tree, source: &str) -> Module {
        lower_tree(tree, source, self)
    }

    pub fn lower_root(&mut self, root: Node<'_>, rope: &Rope) -> Module {
        lower_root(root, rope, self)
    }

    pub fn lower_node(&mut self, node: Node<'_>, source: &str) -> Expression {
        lower_node(node, source, self)
    }

    pub fn lower_node_with_rope(&mut self, node: Node<'_>, rope: &Rope) -> Expression {
        lower_node_with_rope(node, rope, self)
    }
}

pub fn lower_tree(tree: &Tree, source: &str, lowering_context: &mut LoweringContext) -> Module {
    let root = tree.root_node();
    let rope = Rope::from_str(source);
    lower_root(root, &rope, lowering_context)
}

pub fn lower_root(root: Node<'_>, rope: &Rope, lowering_context: &mut LoweringContext) -> Module {
    let mut expressions = Vec::new();

    let child_count = root.named_child_count();
    for child_index in 0..child_count {
        if let Some(child) = root.named_child(child_index) {
            if child.kind() == "comment" {
                continue;
            }
            expressions.push(lower_node_with_rope(child, rope, lowering_context));
        }
    }

    let annotations = collect_pending_annotations(rope, lowering_context);

    attach_annotations_to_expressions(&annotations, &mut expressions);

    Module::new(expressions, annotations)
}

pub fn lower_node(
    node: Node<'_>,
    source: &str,
    lowering_context: &mut LoweringContext,
) -> Expression {
    let rope = Rope::from_str(source);
    lower_node_with_rope(node, &rope, lowering_context)
}

pub fn lower_node_with_rope(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> Expression {
    let kind = match node.kind() {
        "identifier" => ExpressionKind::Symbol(intern_node_text(node, rope, lowering_context)),
        "null" => ExpressionKind::Null,
        "true" => ExpressionKind::Logical(true),
        "false" => ExpressionKind::Logical(false),
        "integer" => ExpressionKind::Integer(node_text(node, rope)),
        "float" => ExpressionKind::Double(node_text(node, rope)),
        "string" => ExpressionKind::Character(node_text(node, rope)),
        "braced_expression" => lower_block(node, rope, lowering_context),
        "binary_operator" => lower_binary_operator(node, rope, lowering_context),
        "unary_operator" => lower_unary_operator(node, rope, lowering_context),
        "function_definition" => lower_function_definition(node, rope, lowering_context),
        "if_statement" => lower_if_statement(node, rope, lowering_context),
        "for_statement" => lower_for_statement(node, rope, lowering_context),
        "while_statement" => lower_while_statement(node, rope, lowering_context),
        "repeat_statement" => lower_repeat_statement(node, rope, lowering_context),
        "call" => lower_call(node, rope, lowering_context),
        "subset" => lower_subset(node, rope, lowering_context),
        "subset2" => lower_subset2(node, rope, lowering_context),
        "extract_operator" => lower_extract_operator(node, rope, lowering_context),
        "parenthesized_expression" => lower_wrapped_expression_kind(node, rope, lowering_context),
        _ => ExpressionKind::Unsupported,
    };

    lowering_context.annotated_expression(node.range(), None, kind)
}

fn lower_binary_operator(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let maybe_lhs = node.child_by_field_name("lhs");
    let maybe_operator = node.child_by_field_name("operator");
    let maybe_rhs = node.child_by_field_name("rhs");

    let Some(lhs) = maybe_lhs else {
        return ExpressionKind::Unsupported;
    };
    let Some(operator) = maybe_operator else {
        return ExpressionKind::Unsupported;
    };
    let Some(rhs) = maybe_rhs else {
        return ExpressionKind::Unsupported;
    };

    match operator.kind() {
        "<-" | "=" => {
            if lhs.kind() != "identifier" {
                return ExpressionKind::Unsupported;
            }

            let target = intern_node_text(lhs, rope, lowering_context);
            let value = lower_node_with_rope(rhs, rope, lowering_context);

            ExpressionKind::Assign {
                target,
                annotation: None,
                value: Box::new(value),
            }
        }
        "+" | "-" | "*" | "/" | "**" | "&&" | "||" => {
            let operator_symbol = intern_node_text(operator, rope, lowering_context);
            let callee = Box::new(
                lowering_context
                    .expression(operator.range(), ExpressionKind::Symbol(operator_symbol)),
            );
            let arguments = vec![
                Argument {
                    expression: lower_node_with_rope(lhs, rope, lowering_context),
                    name: None,
                },
                Argument {
                    expression: lower_node_with_rope(rhs, rope, lowering_context),
                    name: None,
                },
            ];

            ExpressionKind::Call { callee, arguments }
        }
        _ => ExpressionKind::Unsupported,
    }
}

fn lower_block(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let mut expressions = Vec::new();

    for child_index in 0..node.named_child_count() {
        let Some(child) = node.named_child(child_index) else {
            continue;
        };
        expressions.push(lower_node_with_rope(child, rope, lowering_context));
    }

    let has_trailing_semicolon = node_text(node, rope)
        .trim()
        .strip_suffix('}')
        .is_some_and(|prefix| prefix.trim_end().ends_with(';'));

    ExpressionKind::Block {
        expressions,
        has_trailing_semicolon,
    }
}

fn lower_unary_operator(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(operator) = node.child_by_field_name("operator") else {
        return ExpressionKind::Unsupported;
    };
    let Some(value) = node.child_by_field_name("rhs") else {
        return ExpressionKind::Unsupported;
    };

    match operator.kind() {
        "-" => ExpressionKind::UnaryMinus {
            value: Box::new(lower_node_with_rope(value, rope, lowering_context)),
        },
        _ => ExpressionKind::Unsupported,
    }
}

fn lower_function_definition(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let parameters = node
        .child_by_field_name("parameters")
        .map(|parameters| lower_parameters(parameters, rope, lowering_context))
        .unwrap_or_default();

    let body = node
        .child_by_field_name("body")
        .map(|body| Box::new(lower_node_with_rope(body, rope, lowering_context)))
        .unwrap_or_else(|| {
            Box::new(lowering_context.expression(node.range(), ExpressionKind::Unsupported))
        });

    ExpressionKind::Function { parameters, body }
}

fn lower_if_statement(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(condition) = node.child_by_field_name("condition") else {
        return ExpressionKind::Unsupported;
    };
    let Some(consequence) = node.child_by_field_name("consequence") else {
        return ExpressionKind::Unsupported;
    };

    ExpressionKind::If {
        condition: Box::new(lower_node_with_rope(condition, rope, lowering_context)),
        consequence: Box::new(lower_node_with_rope(consequence, rope, lowering_context)),
        alternative: node
            .child_by_field_name("alternative")
            .map(|alternative| Box::new(lower_node_with_rope(alternative, rope, lowering_context))),
    }
}

fn lower_for_statement(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(variable) = node.child_by_field_name("variable") else {
        return ExpressionKind::Unsupported;
    };
    if variable.kind() != "identifier" {
        return ExpressionKind::Unsupported;
    }
    let Some(sequence) = node.child_by_field_name("sequence") else {
        return ExpressionKind::Unsupported;
    };
    let Some(body) = node.child_by_field_name("body") else {
        return ExpressionKind::Unsupported;
    };

    ExpressionKind::For {
        variable: intern_node_text(variable, rope, lowering_context),
        sequence: Box::new(lower_node_with_rope(sequence, rope, lowering_context)),
        body: Box::new(lower_node_with_rope(body, rope, lowering_context)),
    }
}

fn lower_while_statement(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(condition) = node.child_by_field_name("condition") else {
        return ExpressionKind::Unsupported;
    };
    let Some(body) = node.child_by_field_name("body") else {
        return ExpressionKind::Unsupported;
    };

    ExpressionKind::While {
        condition: Box::new(lower_node_with_rope(condition, rope, lowering_context)),
        body: Box::new(lower_node_with_rope(body, rope, lowering_context)),
    }
}

fn lower_repeat_statement(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(body) = node.child_by_field_name("body") else {
        return ExpressionKind::Unsupported;
    };

    ExpressionKind::Repeat {
        body: Box::new(lower_node_with_rope(body, rope, lowering_context)),
    }
}

fn lower_parameters(
    parameters: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> Vec<Parameter> {
    let mut lowered_parameters = Vec::new();
    let child_count = parameters.named_child_count();

    for child_index in 0..child_count {
        let Some(child) = parameters.named_child(child_index) else {
            continue;
        };

        match child.kind() {
            "identifier" => {
                lowered_parameters.push(Parameter {
                    symbol: intern_node_text(child, rope, lowering_context),
                    range: child.range(),
                });
            }
            "parameter" => {
                if let Some(name) = child.child_by_field_name("name")
                    && name.kind() == "identifier"
                {
                    lowered_parameters.push(Parameter {
                        symbol: intern_node_text(name, rope, lowering_context),
                        range: name.range(),
                    });
                }
            }
            _ => {}
        }
    }

    lowered_parameters
}

fn lower_call(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(function) = node.child_by_field_name("function") else {
        return ExpressionKind::Unsupported;
    };

    let callee = Box::new(lower_node_with_rope(function, rope, lowering_context));
    let arguments = node
        .child_by_field_name("arguments")
        .map(|arguments| lower_arguments(arguments, rope, lowering_context))
        .unwrap_or_default();

    ExpressionKind::Call { callee, arguments }
}

fn lower_subset(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(function) = node.child_by_field_name("function") else {
        return ExpressionKind::Unsupported;
    };

    let value = Box::new(lower_node_with_rope(function, rope, lowering_context));
    let arguments = node
        .child_by_field_name("arguments")
        .map(|arguments| lower_index_arguments(arguments, rope, lowering_context))
        .unwrap_or_default();

    ExpressionKind::Subset { value, arguments }
}

fn lower_subset2(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(function) = node.child_by_field_name("function") else {
        return ExpressionKind::Unsupported;
    };

    let value = Box::new(lower_node_with_rope(function, rope, lowering_context));
    let arguments = node
        .child_by_field_name("arguments")
        .map(|arguments| lower_index_arguments(arguments, rope, lowering_context))
        .unwrap_or_default();

    ExpressionKind::Subset2 { value, arguments }
}

fn lower_extract_operator(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(operator) = node.child_by_field_name("operator") else {
        return ExpressionKind::Unsupported;
    };
    if operator.kind() != "$" {
        return ExpressionKind::Unsupported;
    }

    let Some(lhs) = node.child_by_field_name("lhs") else {
        return ExpressionKind::Unsupported;
    };
    let Some(rhs) = node.child_by_field_name("rhs") else {
        return ExpressionKind::Unsupported;
    };
    let name = match rhs.kind() {
        "identifier" => intern_node_text(rhs, rope, lowering_context),
        "string" => intern_string_node_content(rhs, rope, lowering_context),
        _ => return ExpressionKind::Unsupported,
    };

    ExpressionKind::Dollar {
        value: Box::new(lower_node_with_rope(lhs, rope, lowering_context)),
        name,
    }
}

fn lower_arguments(
    arguments: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> Vec<Argument> {
    let mut lowered_arguments = Vec::new();
    let child_count = arguments.named_child_count();

    for child_index in 0..child_count {
        let Some(child) = arguments.named_child(child_index) else {
            continue;
        };

        if child.kind() != "argument" {
            continue;
        }

        let name = child
            .child_by_field_name("name")
            .filter(|name| name.kind() == "identifier")
            .map(|name| intern_node_text(name, rope, lowering_context));

        let expression = child
            .child_by_field_name("value")
            .map(|value| lower_node_with_rope(value, rope, lowering_context))
            .unwrap_or_else(|| {
                lowering_context.expression(child.range(), ExpressionKind::Unsupported)
            });

        lowered_arguments.push(Argument { expression, name });
    }

    lowered_arguments
}

fn lower_index_arguments(
    arguments: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> Vec<Argument> {
    let mut lowered_arguments = Vec::new();
    let child_count = arguments.named_child_count();

    for child_index in 0..child_count {
        let Some(child) = arguments.named_child(child_index) else {
            continue;
        };

        if child.kind() != "argument" {
            continue;
        }

        let name = child
            .child_by_field_name("name")
            .filter(|name| name.kind() == "identifier")
            .map(|name| intern_node_text(name, rope, lowering_context));

        let expression = match child.child_by_field_name("value") {
            None => lowering_context.expression(child.range(), ExpressionKind::Unsupported),
            Some(value) if value.kind() == "string" => {
                let symbol = intern_string_node_content(value, rope, lowering_context);
                lowering_context
                    .expression(value.range(), ExpressionKind::StringLiteralName(symbol))
            }
            Some(value) => lower_node_with_rope(value, rope, lowering_context),
        };

        lowered_arguments.push(Argument { expression, name });
    }

    lowered_arguments
}

fn lower_wrapped_expression_kind(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    if let Some(inner) = first_named_child(node) {
        return lower_node_with_rope(inner, rope, lowering_context).kind;
    }

    ExpressionKind::Unsupported
}

fn first_named_child(node: Node<'_>) -> Option<Node<'_>> {
    let child_count = node.named_child_count();

    for child_index in 0..child_count {
        if let Some(child) = node.named_child(child_index) {
            return Some(child);
        }
    }

    None
}

fn node_text(node: Node<'_>, rope: &Rope) -> String {
    text::node_text(rope, node).unwrap_or_default()
}

fn intern_string_node_content(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> Symbol {
    if let Some(content) = node.child_by_field_name("content") {
        return intern_node_text(content, rope, lowering_context);
    }

    let text = node_text(node, rope);
    let Some(content) = text.strip_prefix(text.chars().next().unwrap_or('"')) else {
        return lowering_context.intern(&text);
    };
    let content = content
        .strip_suffix(text.chars().last().unwrap_or('"'))
        .unwrap_or(content);
    lowering_context.intern(content)
}

fn intern_node_text(node: Node<'_>, rope: &Rope, lowering_context: &mut LoweringContext) -> Symbol {
    let text = node_text(node, rope);
    lowering_context.intern(&text)
}

fn collect_pending_annotations(
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> Vec<PendingAnnotation> {
    let mut annotations = Vec::new();
    let lines = text::all_lines(rope);
    let mut row = 0;

    while row < lines.len() {
        let line = &lines[row];
        let trimmed_line = line.trim_start();
        let Some(annotation_text) = trimmed_line.strip_prefix("#:") else {
            row += 1;
            continue;
        };
        let start_column = line.len() - trimmed_line.len();
        let _ = annotation_text;
        let (annotation_block_text, last_row) = text::annotation_block_text(rope, row);

        if let Ok(annotation) =
            parse_annotation(&annotation_block_text, lowering_context.interner_mut())
        {
            let end_column = text::line_text(rope, last_row)
                .map(|line_text| line_text.len())
                .unwrap_or(0);
            annotations.push(PendingAnnotation {
                range: Range {
                    start_byte: 0,
                    end_byte: 0,
                    start_point: tree_sitter::Point {
                        row,
                        column: start_column,
                    },
                    end_point: tree_sitter::Point {
                        row: last_row,
                        column: end_column,
                    },
                },
                annotation,
            });
        }

        row = last_row + 1;
    }

    annotations
}

fn attach_annotations_to_expressions(
    annotations: &[PendingAnnotation],
    expressions: &mut [Expression],
) {
    for pending_annotation in annotations {
        if let Some(expression) = trailing_top_level_expression(expressions, pending_annotation) {
            attach_annotation_to_expression(expression, &pending_annotation.annotation);
        }
    }
}

fn trailing_top_level_expression<'a>(
    expressions: &'a mut [Expression],
    pending_annotation: &PendingAnnotation,
) -> Option<&'a mut Expression> {
    let annotation_row = pending_annotation.range.start_point.row;

    expressions
        .iter_mut()
        .find(|expression| expression.range.start_point.row > annotation_row)
}

fn attach_annotation_to_expression(expression: &mut Expression, annotation: &Annotation) {
    expression.annotation = Some(AttachedAnnotation::expression(annotation.clone()));

    if let ExpressionKind::Assign {
        annotation: assignment_annotation,
        ..
    } = &mut expression.kind
    {
        *assignment_annotation = Some(AttachedAnnotation::binding_and_expression(
            annotation.clone(),
        ));
    }
}
