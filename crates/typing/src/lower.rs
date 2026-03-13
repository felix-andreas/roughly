use {
    crate::interner::{Interner, Symbol},
    tree_sitter::{Node, Range, Tree},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExpressionId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub expressions: Vec<Expression>,
}

impl Module {
    pub fn new(expressions: Vec<Expression>) -> Self {
        Self { expressions }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expression {
    pub id: ExpressionId,
    pub range: Range,
    pub kind: ExpressionKind,
}

impl Expression {
    pub fn new(id: ExpressionId, range: Range, kind: ExpressionKind) -> Self {
        Self { id, range, kind }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionKind {
    Null,
    Logical(bool),
    Integer(String),
    Double(String),
    Character(String),
    Symbol(Symbol),
    Assign {
        target: Symbol,
        value: Box<Expression>,
    },
    Function {
        parameters: Vec<Parameter>,
        body: Box<Expression>,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Argument>,
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
        Expression::new(self.fresh_expression_id(), range, kind)
    }

    pub fn lower_tree(&mut self, tree: &Tree, source: &str) -> Module {
        lower_tree(self, tree, source)
    }

    pub fn lower_node(&mut self, node: Node<'_>, source: &str) -> Expression {
        lower_node(self, node, source)
    }
}

pub fn lower_tree(lowering_context: &mut LoweringContext, tree: &Tree, source: &str) -> Module {
    let root = tree.root_node();
    let mut expressions = Vec::new();

    let child_count = root.named_child_count();
    for child_index in 0..child_count {
        if let Some(child) = root.named_child(child_index) {
            expressions.push(lower_node(lowering_context, child, source));
        }
    }

    Module::new(expressions)
}

pub fn lower_node(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> Expression {
    let kind = match node.kind() {
        "identifier" => ExpressionKind::Symbol(intern_node_text(lowering_context, node, source)),
        "null" => ExpressionKind::Null,
        "true" => ExpressionKind::Logical(true),
        "false" => ExpressionKind::Logical(false),
        "integer" => ExpressionKind::Integer(node_text(node, source).to_owned()),
        "float" => ExpressionKind::Double(node_text(node, source).to_owned()),
        "string" => ExpressionKind::Character(node_text(node, source).to_owned()),
        "binary_operator" => lower_binary_operator(lowering_context, node, source),
        "function_definition" => lower_function_definition(lowering_context, node, source),
        "call" => lower_call(lowering_context, node, source),
        "parenthesized_expression" => lower_wrapped_expression_kind(lowering_context, node, source),
        "braced_expression" => lower_wrapped_expression_kind(lowering_context, node, source),
        _ => ExpressionKind::Unsupported,
    };

    lowering_context.expression(node.range(), kind)
}

fn lower_binary_operator(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
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

    if !matches!(operator.kind(), "<-" | "=") {
        return ExpressionKind::Unsupported;
    }

    if lhs.kind() != "identifier" {
        return ExpressionKind::Unsupported;
    }

    let target = intern_node_text(lowering_context, lhs, source);
    let value = lower_node(lowering_context, rhs, source);

    ExpressionKind::Assign {
        target,
        value: Box::new(value),
    }
}

fn lower_function_definition(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> ExpressionKind {
    let parameters = node
        .child_by_field_name("parameters")
        .map(|parameters| lower_parameters(lowering_context, parameters, source))
        .unwrap_or_default();

    let body = node
        .child_by_field_name("body")
        .map(|body| Box::new(lower_node(lowering_context, body, source)))
        .unwrap_or_else(|| {
            Box::new(lowering_context.expression(node.range(), ExpressionKind::Unsupported))
        });

    ExpressionKind::Function { parameters, body }
}

fn lower_parameters(
    lowering_context: &mut LoweringContext,
    parameters: Node<'_>,
    source: &str,
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
                    symbol: intern_node_text(lowering_context, child, source),
                    range: child.range(),
                });
            }
            "parameter" => {
                if let Some(name) = child.child_by_field_name("name")
                    && name.kind() == "identifier"
                {
                    lowered_parameters.push(Parameter {
                        symbol: intern_node_text(lowering_context, name, source),
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
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> ExpressionKind {
    let Some(function) = node.child_by_field_name("function") else {
        return ExpressionKind::Unsupported;
    };

    let callee = Box::new(lower_node(lowering_context, function, source));
    let arguments = node
        .child_by_field_name("arguments")
        .map(|arguments| lower_arguments(lowering_context, arguments, source))
        .unwrap_or_default();

    ExpressionKind::Call { callee, arguments }
}

fn lower_arguments(
    lowering_context: &mut LoweringContext,
    arguments: Node<'_>,
    source: &str,
) -> Vec<Argument> {
    let mut lowered_arguments = Vec::new();
    let child_count = arguments.named_child_count();

    for child_index in 0..child_count {
        let Some(child) = arguments.named_child(child_index) else {
            continue;
        };

        if child.kind() == "argument" {
            let name = child
                .child_by_field_name("name")
                .filter(|name| name.kind() == "identifier")
                .map(|name| intern_node_text(lowering_context, name, source));

            let expression = child
                .child_by_field_name("value")
                .map(|value| lower_node(lowering_context, value, source))
                .unwrap_or_else(|| {
                    lowering_context.expression(child.range(), ExpressionKind::Unsupported)
                });

            lowered_arguments.push(Argument { expression, name });
            continue;
        }

        lowered_arguments.push(Argument {
            expression: lower_node(lowering_context, child, source),
            name: None,
        });
    }

    lowered_arguments
}

fn lower_wrapped_expression_kind(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> ExpressionKind {
    if let Some(inner) = first_named_child(node) {
        return lower_node(lowering_context, inner, source).kind;
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

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn intern_node_text(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> Symbol {
    lowering_context.intern(node_text(node, source))
}
