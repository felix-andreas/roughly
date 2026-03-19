use {
    crate::{
        interner::{Interner, Symbol},
        surface_types::parse_annotation,
        types::{
            Annotation, AnnotationKind, AttachedAnnotation, FunctionType, RecordField, SurfaceType,
        },
    },
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
            if child.kind() == "comment" {
                continue;
            }
            expressions.push(lower_node(lowering_context, child, source));
        }
    }

    let annotations = collect_pending_annotations(lowering_context, source);

    attach_annotations_to_expressions(&annotations, &mut expressions);

    Module::new(expressions, annotations)
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
        "braced_expression" => lower_block(lowering_context, node, source),
        "binary_operator" => lower_binary_operator(lowering_context, node, source),
        "unary_operator" => lower_unary_operator(lowering_context, node, source),
        "function_definition" => lower_function_definition(lowering_context, node, source),
        "if_statement" => lower_if_statement(lowering_context, node, source),
        "for_statement" => lower_for_statement(lowering_context, node, source),
        "while_statement" => lower_while_statement(lowering_context, node, source),
        "repeat_statement" => lower_repeat_statement(lowering_context, node, source),
        "call" => lower_call(lowering_context, node, source),
        "subset" => lower_subset(lowering_context, node, source),
        "subset2" => lower_subset2(lowering_context, node, source),
        "extract_operator" => lower_extract_operator(lowering_context, node, source),
        "parenthesized_expression" => lower_wrapped_expression_kind(lowering_context, node, source),
        _ => ExpressionKind::Unsupported,
    };

    lowering_context.annotated_expression(node.range(), None, kind)
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

    match operator.kind() {
        "<-" | "=" => {
            if lhs.kind() != "identifier" {
                return ExpressionKind::Unsupported;
            }

            let target = intern_node_text(lowering_context, lhs, source);
            let value = lower_node(lowering_context, rhs, source);

            ExpressionKind::Assign {
                target,
                annotation: None,
                value: Box::new(value),
            }
        }
        "+" | "-" | "*" | "/" | "**" | "&&" | "||" => {
            let operator_symbol = intern_node_text(lowering_context, operator, source);
            let callee = Box::new(
                lowering_context
                    .expression(operator.range(), ExpressionKind::Symbol(operator_symbol)),
            );
            let arguments = vec![
                Argument {
                    expression: lower_node(lowering_context, lhs, source),
                    name: None,
                },
                Argument {
                    expression: lower_node(lowering_context, rhs, source),
                    name: None,
                },
            ];

            ExpressionKind::Call { callee, arguments }
        }
        _ => ExpressionKind::Unsupported,
    }
}

fn lower_block(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> ExpressionKind {
    let mut expressions = Vec::new();

    for child_index in 0..node.named_child_count() {
        let Some(child) = node.named_child(child_index) else {
            continue;
        };
        expressions.push(lower_node(lowering_context, child, source));
    }

    let has_trailing_semicolon = node_text(node, source)
        .trim()
        .strip_suffix('}')
        .is_some_and(|prefix| prefix.trim_end().ends_with(';'));

    ExpressionKind::Block {
        expressions,
        has_trailing_semicolon,
    }
}

fn lower_unary_operator(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> ExpressionKind {
    let Some(operator) = node.child_by_field_name("operator") else {
        return ExpressionKind::Unsupported;
    };
    let Some(value) = node.child_by_field_name("rhs") else {
        return ExpressionKind::Unsupported;
    };

    match operator.kind() {
        "-" => ExpressionKind::UnaryMinus {
            value: Box::new(lower_node(lowering_context, value, source)),
        },
        _ => ExpressionKind::Unsupported,
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

fn lower_if_statement(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> ExpressionKind {
    let Some(condition) = node.child_by_field_name("condition") else {
        return ExpressionKind::Unsupported;
    };
    let Some(consequence) = node.child_by_field_name("consequence") else {
        return ExpressionKind::Unsupported;
    };

    ExpressionKind::If {
        condition: Box::new(lower_node(lowering_context, condition, source)),
        consequence: Box::new(lower_node(lowering_context, consequence, source)),
        alternative: node
            .child_by_field_name("alternative")
            .map(|alternative| Box::new(lower_node(lowering_context, alternative, source))),
    }
}

fn lower_for_statement(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
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
        variable: intern_node_text(lowering_context, variable, source),
        sequence: Box::new(lower_node(lowering_context, sequence, source)),
        body: Box::new(lower_node(lowering_context, body, source)),
    }
}

fn lower_while_statement(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> ExpressionKind {
    let Some(condition) = node.child_by_field_name("condition") else {
        return ExpressionKind::Unsupported;
    };
    let Some(body) = node.child_by_field_name("body") else {
        return ExpressionKind::Unsupported;
    };

    ExpressionKind::While {
        condition: Box::new(lower_node(lowering_context, condition, source)),
        body: Box::new(lower_node(lowering_context, body, source)),
    }
}

fn lower_repeat_statement(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> ExpressionKind {
    let Some(body) = node.child_by_field_name("body") else {
        return ExpressionKind::Unsupported;
    };

    ExpressionKind::Repeat {
        body: Box::new(lower_node(lowering_context, body, source)),
    }
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

fn lower_subset(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> ExpressionKind {
    let Some(function) = node.child_by_field_name("function") else {
        return ExpressionKind::Unsupported;
    };

    let value = Box::new(lower_node(lowering_context, function, source));
    let arguments = node
        .child_by_field_name("arguments")
        .map(|arguments| lower_index_arguments(lowering_context, arguments, source))
        .unwrap_or_default();

    ExpressionKind::Subset { value, arguments }
}

fn lower_subset2(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> ExpressionKind {
    let Some(function) = node.child_by_field_name("function") else {
        return ExpressionKind::Unsupported;
    };

    let value = Box::new(lower_node(lowering_context, function, source));
    let arguments = node
        .child_by_field_name("arguments")
        .map(|arguments| lower_index_arguments(lowering_context, arguments, source))
        .unwrap_or_default();

    ExpressionKind::Subset2 { value, arguments }
}

fn lower_extract_operator(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
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
        "identifier" => intern_node_text(lowering_context, rhs, source),
        "string" => intern_string_node_content(lowering_context, rhs, source),
        _ => return ExpressionKind::Unsupported,
    };

    ExpressionKind::Dollar {
        value: Box::new(lower_node(lowering_context, lhs, source)),
        name,
    }
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

        if child.kind() != "argument" {
            continue;
        }

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
    }

    lowered_arguments
}

fn lower_index_arguments(
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

        if child.kind() != "argument" {
            continue;
        }

        let name = child
            .child_by_field_name("name")
            .filter(|name| name.kind() == "identifier")
            .map(|name| intern_node_text(lowering_context, name, source));

        let expression = match child.child_by_field_name("value") {
            None => lowering_context.expression(child.range(), ExpressionKind::Unsupported),
            Some(value) if value.kind() == "string" => {
                let symbol = intern_string_node_content(lowering_context, value, source);
                lowering_context
                    .expression(value.range(), ExpressionKind::StringLiteralName(symbol))
            }
            Some(value) => lower_node(lowering_context, value, source),
        };

        lowered_arguments.push(Argument { expression, name });
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

fn intern_string_node_content(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> Symbol {
    if let Some(content) = node.child_by_field_name("content") {
        return intern_node_text(lowering_context, content, source);
    }

    let text = node_text(node, source);
    let Some(content) = text.strip_prefix(text.chars().next().unwrap_or('"')) else {
        return lowering_context.intern(text);
    };
    let content = content
        .strip_suffix(text.chars().last().unwrap_or('"'))
        .unwrap_or(content);
    lowering_context.intern(content)
}

fn intern_node_text(
    lowering_context: &mut LoweringContext,
    node: Node<'_>,
    source: &str,
) -> Symbol {
    lowering_context.intern(node_text(node, source))
}

fn collect_pending_annotations(
    lowering_context: &mut LoweringContext,
    source: &str,
) -> Vec<PendingAnnotation> {
    let mut annotations = Vec::new();
    let lines = source.lines().collect::<Vec<_>>();
    let mut row = 0;

    while row < lines.len() {
        let line = lines[row];
        let trimmed_line = line.trim_start();
        let Some(annotation_text) = trimmed_line.strip_prefix("#:") else {
            row += 1;
            continue;
        };
        let annotation_text = annotation_text.trim();
        let start_column = line.len() - trimmed_line.len();

        if is_expanded_function_annotation_line(annotation_text) {
            if let Some((annotation, last_row)) =
                parse_expanded_function_annotation(lowering_context, &lines, row)
            {
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
                            column: lines[last_row].len(),
                        },
                    },
                    annotation,
                });
                row = last_row + 1;
                continue;
            }
        }

        if let Ok(annotation) = parse_annotation(lowering_context.interner_mut(), annotation_text) {
            annotations.push(PendingAnnotation {
                range: Range {
                    start_byte: 0,
                    end_byte: 0,
                    start_point: tree_sitter::Point {
                        row,
                        column: start_column,
                    },
                    end_point: tree_sitter::Point {
                        row,
                        column: line.len(),
                    },
                },
                annotation,
            });
        }

        row += 1;
    }

    annotations
}

fn is_expanded_function_annotation_line(text: &str) -> bool {
    text.starts_with("@param ") || text.starts_with("@return ") || text.starts_with("@returns ")
}

fn parse_expanded_function_annotation(
    lowering_context: &mut LoweringContext,
    lines: &[&str],
    start_row: usize,
) -> Option<(Annotation, usize)> {
    let mut row = start_row;
    let mut parameters = Vec::new();
    let mut named_parameters = Vec::new();
    let mut return_type = SurfaceType::Null;

    while row < lines.len() {
        let line = lines[row];
        let trimmed_line = line.trim_start();
        let Some(annotation_text) = trimmed_line.strip_prefix("#:") else {
            break;
        };
        let annotation_text = annotation_text.trim();
        if !is_expanded_function_annotation_line(annotation_text) {
            break;
        }

        if let Some((surface_type, name_text)) =
            parse_expanded_param_annotation(lowering_context, annotation_text)
        {
            let name = lowering_context.intern(&name_text);
            named_parameters.push(RecordField::new(name, surface_type));
        } else if let Some(surface_type) =
            parse_expanded_return_annotation(lowering_context, annotation_text)
        {
            return_type = surface_type;
        } else {
            return None;
        }

        row += 1;
    }

    if row == start_row {
        return None;
    }

    Some((
        Annotation::new(
            AnnotationKind::Checked,
            SurfaceType::Function(FunctionType::new(
                std::mem::take(&mut parameters),
                named_parameters,
                return_type,
            )),
        ),
        row - 1,
    ))
}

fn parse_expanded_param_annotation(
    lowering_context: &mut LoweringContext,
    text: &str,
) -> Option<(crate::types::SurfaceType, String)> {
    let parameter_text = text.strip_prefix("@param")?.trim_start();
    let (type_text, name_text) = parse_braced_type_and_tail(parameter_text)?;
    let normalized_name = name_text
        .trim()
        .strip_prefix('[')
        .and_then(|name| name.strip_suffix(']'))
        .unwrap_or_else(|| name_text.trim())
        .to_owned();
    Some((
        crate::surface_types::parse_surface_type(lowering_context.interner_mut(), type_text)
            .ok()?,
        normalized_name,
    ))
}

fn parse_expanded_return_annotation(
    lowering_context: &mut LoweringContext,
    text: &str,
) -> Option<crate::types::SurfaceType> {
    let return_text = text
        .strip_prefix("@return")
        .or_else(|| text.strip_prefix("@returns"))?
        .trim_start();
    let (type_text, trailing_text) = parse_braced_type_and_tail(return_text)?;
    if !trailing_text.trim().is_empty() {
        return None;
    }
    crate::surface_types::parse_surface_type(lowering_context.interner_mut(), type_text).ok()
}

fn parse_braced_type_and_tail(text: &str) -> Option<(&str, &str)> {
    let inner_text = text.strip_prefix('{')?;
    let closing_index = find_matching_closer(inner_text, '{', '}')?;
    let type_text = &inner_text[..closing_index];
    let trailing_text = &inner_text[closing_index + 1..];
    Some((type_text.trim(), trailing_text))
}

fn find_matching_closer(text: &str, opener: char, closer: char) -> Option<usize> {
    let mut depth = 1;

    for (index, character) in text.char_indices() {
        if character == opener {
            depth += 1;
        } else if character == closer {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }

    None
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
