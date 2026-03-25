use {
    crate::{
        annotations::{TypeParseError, TypeSyntaxItem, parse_annotation},
        diagnostic::Diagnostic,
        hir::{
            Argument, Definition, Expression, ExpressionId, ExpressionKind, HirArena, Module,
            Parameter,
        },
        interner::{Interner, Symbol},
        text,
        types::{Annotation, AttachedAnnotation, SurfaceType},
    },
    ropey::Rope,
    std::collections::BTreeSet,
    tree_sitter::{Node, Range, Tree},
};

#[derive(Debug, Default)]
pub struct LoweringContext {
    pub arena: HirArena,
    pub diagnostics: Vec<Diagnostic>,
    declared_type_names: BTreeSet<Symbol>,
    interner: Interner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweringResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

impl LoweringContext {
    pub fn new() -> Self {
        Self::default()
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

    pub fn expression(&mut self, range: Range, kind: ExpressionKind) -> ExpressionId {
        self.annotated_expression(range, None, kind)
    }

    pub fn annotated_expression(
        &mut self,
        range: Range,
        annotation: Option<AttachedAnnotation>,
        kind: ExpressionKind,
    ) -> ExpressionId {
        // ID will be assigned by arena.alloc
        let expr = Expression::new(ExpressionId(0), range, annotation, kind);
        self.arena.alloc(expr)
    }

    pub fn lower_tree(&mut self, tree: &Tree, source: &str) -> Module {
        lower_tree(tree, source, self)
    }

    pub fn lower_tree_with_diagnostics(&mut self, tree: &Tree, source: &str) -> LoweringResult {
        lower_tree_with_diagnostics(tree, source, self)
    }

    pub fn lower_root_with_rope(&mut self, root: Node<'_>, rope: &Rope) -> Module {
        lower_root_with_rope(root, rope, self)
    }

    pub fn lower_root_with_rope_with_diagnostics(
        &mut self,
        root: Node<'_>,
        rope: &Rope,
    ) -> LoweringResult {
        lower_root_with_rope_with_diagnostics(root, rope, self)
    }

    pub fn lower_node(&mut self, node: Node<'_>, source: &str) -> ExpressionId {
        lower_node(node, source, self)
    }

    pub fn lower_node_with_rope(&mut self, node: Node<'_>, rope: &Rope) -> ExpressionId {
        lower_node_with_rope(node, rope, self)
    }
}

pub fn lower_tree(tree: &Tree, source: &str, lowering_context: &mut LoweringContext) -> Module {
    lower_tree_with_diagnostics(tree, source, lowering_context).module
}

pub fn lower_tree_with_diagnostics(
    tree: &Tree,
    source: &str,
    lowering_context: &mut LoweringContext,
) -> LoweringResult {
    let root = tree.root_node();
    let rope = Rope::from_str(source);
    lower_root_with_rope_with_diagnostics(root, &rope, lowering_context)
}

pub fn lower_root_with_rope(
    root: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> Module {
    lower_root_with_rope_with_diagnostics(root, rope, lowering_context).module
}

pub fn lower_root_with_rope_with_diagnostics(
    root: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> LoweringResult {
    lowering_context.declared_type_names.clear();
    let expressions = lower_expression_list(root, rope, lowering_context);
    lowering_context.declared_type_names.clear();

    let arena = std::mem::take(&mut lowering_context.arena);
    let diagnostics = std::mem::take(&mut lowering_context.diagnostics);

    LoweringResult {
        module: Module::new(arena, expressions),
        diagnostics,
    }
}

pub fn lower_node(
    node: Node<'_>,
    source: &str,
    lowering_context: &mut LoweringContext,
) -> ExpressionId {
    let rope = Rope::from_str(source);
    lower_node_with_rope(node, &rope, lowering_context)
}

pub fn lower_node_with_rope(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionId {
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
        "parenthesized_expression" => {
            if let Some(inner) = first_named_child(node) {
                return lower_node_with_rope(inner, rope, lowering_context);
            }
            ExpressionKind::Unsupported
        }
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
                value,
            }
        }
        "+" | "-" | "*" | "/" | "**" | "&&" | "||" => {
            let operator_symbol = intern_node_text(operator, rope, lowering_context);
            let callee = lowering_context
                .expression(operator.range(), ExpressionKind::Symbol(operator_symbol));
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
    let expressions = lower_expression_list(node, rope, lowering_context);

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
            value: lower_node_with_rope(value, rope, lowering_context),
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
        .map(|body| lower_node_with_rope(body, rope, lowering_context))
        .unwrap_or_else(|| lowering_context.expression(node.range(), ExpressionKind::Unsupported));

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
        condition: lower_node_with_rope(condition, rope, lowering_context),
        consequence: lower_node_with_rope(consequence, rope, lowering_context),
        alternative: node
            .child_by_field_name("alternative")
            .map(|alternative| lower_node_with_rope(alternative, rope, lowering_context)),
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
        sequence: lower_node_with_rope(sequence, rope, lowering_context),
        body: lower_node_with_rope(body, rope, lowering_context),
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
        condition: lower_node_with_rope(condition, rope, lowering_context),
        body: lower_node_with_rope(body, rope, lowering_context),
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
        body: lower_node_with_rope(body, rope, lowering_context),
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

    let callee = lower_node_with_rope(function, rope, lowering_context);
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

    let value = lower_node_with_rope(function, rope, lowering_context);
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

    let value = lower_node_with_rope(function, rope, lowering_context);
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
        value: lower_node_with_rope(lhs, rope, lowering_context),
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

fn lower_expression_list(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> Vec<ExpressionId> {
    let mut expressions = Vec::new();
    let child_count = node.named_child_count();
    let mut child_index = 0;

    while child_index < child_count {
        let Some(child) = node.named_child(child_index) else {
            child_index += 1;
            continue;
        };

        if child.kind() == "comment" {
            let text = node_text(child, rope);
            let trimmed = text.trim_start();
            if trimmed.starts_with("#:") {
                let mut block_text = trimmed.to_string();
                let mut block_range = child.range();
                let mut block_lines = vec![(trimmed.to_string(), child.range())];

                let mut next_index = child_index + 1;
                while next_index < child_count {
                    if let Some(next_child) = node.named_child(next_index)
                        && next_child.kind() == "comment"
                    {
                        let next_text = node_text(next_child, rope);
                        let next_trimmed = next_text.trim_start();
                        if next_trimmed.starts_with("#:") {
                            if next_child.range().start_point.row > block_range.end_point.row + 1 {
                                break;
                            }
                            block_text.push('\n');
                            block_text.push_str(next_trimmed);
                            block_range.end_point = next_child.range().end_point;
                            block_range.end_byte = next_child.range().end_byte;
                            block_lines.push((next_trimmed.to_string(), next_child.range()));
                            next_index += 1;
                            continue;
                        }
                    }
                    break;
                }

                let stripped_lines = block_lines
                    .iter()
                    .map(|(line, _)| {
                        line.trim()
                            .strip_prefix("#:")
                            .map(str::trim)
                            .unwrap_or(line.trim())
                            .to_owned()
                    })
                    .collect::<Vec<_>>();
                let stripped_text = stripped_lines.join("\n");

                let parsed_annotation = if stripped_text.trim().is_empty() {
                    AnnotationParseOutcome::MissingTypeExpression
                } else {
                    match parse_annotation(&stripped_text, lowering_context.interner_mut()) {
                        Ok(TypeSyntaxItem::Definitions(definitions)) => {
                            AnnotationParseOutcome::Definitions(definitions)
                        }
                        Ok(TypeSyntaxItem::Annotation(annotation)) => match validate_annotation(
                            &annotation,
                            &lowering_context.declared_type_names,
                            lowering_context.interner(),
                        ) {
                            Ok(()) => AnnotationParseOutcome::Annotation(annotation),
                            Err(error) => AnnotationParseOutcome::Error(error),
                        },
                        Err(error) => AnnotationParseOutcome::Error(error),
                    }
                };

                let parsed_annotation = match parsed_annotation {
                    AnnotationParseOutcome::Definitions(definitions) => {
                        for (definition, range) in definitions
                            .into_iter()
                            .zip(block_lines.iter().map(|(_, range)| *range))
                        {
                            let Definition {
                                name,
                                type_parameters,
                                surface_type,
                                kind,
                            } = definition;

                            match validate_declared_type_surface_type(
                                &surface_type,
                                &lowering_context.declared_type_names,
                                &type_parameters,
                                lowering_context.interner(),
                            ) {
                                Ok(()) => {
                                    let kind = ExpressionKind::Definition(Definition {
                                        kind,
                                        name,
                                        type_parameters,
                                        surface_type,
                                    });
                                    lowering_context.declared_type_names.insert(name);
                                    let expr_id =
                                        lowering_context.annotated_expression(range, None, kind);
                                    expressions.push(expr_id);
                                }
                                Err(error) => lowering_context
                                    .diagnostics
                                    .push(annotation_parse_diagnostic(range, error)),
                            }
                        }
                        child_index = next_index;
                        continue;
                    }
                    other => other,
                };

                let next_named_child = if next_index < child_count {
                    node.named_child(next_index)
                } else {
                    None
                };

                if let Some(expr_child) = next_named_child {
                    if expr_child.range().start_point.row > block_range.end_point.row + 1 {
                        lowering_context.diagnostics.push(Diagnostic::syntax_error(
                            block_range,
                            "A `#:` typing comment cannot be separated from its expression by an empty line.",
                        ));
                        child_index = next_index;
                    } else if expr_child.kind() == "comment" {
                        lowering_context.diagnostics.push(Diagnostic::syntax_error(
                            block_range,
                            "A `#:` typing comment must be followed immediately by an expression.",
                        ));
                        child_index = next_index;
                    } else {
                        let expr_id = lower_node_with_rope(expr_child, rope, lowering_context);

                        match parsed_annotation {
                            AnnotationParseOutcome::Annotation(annotation) => {
                                attach_annotation_to_expression(
                                    expr_id,
                                    &annotation,
                                    &mut lowering_context.arena,
                                );
                            }
                            AnnotationParseOutcome::MissingTypeExpression => {
                                lowering_context.diagnostics.push(Diagnostic::syntax_error(
                                    block_range,
                                    "A `#:` typing comment must include a type expression.",
                                ));
                            }
                            AnnotationParseOutcome::Error(error) => {
                                lowering_context
                                    .diagnostics
                                    .push(annotation_parse_diagnostic(block_range, error));
                            }
                            AnnotationParseOutcome::Definitions(_) => {}
                        }

                        expressions.push(expr_id);
                        child_index = next_index + 1;
                    }
                } else {
                    lowering_context.diagnostics.push(Diagnostic::syntax_error(
                        block_range,
                        "A `#:` typing comment must be followed immediately by an expression.",
                    ));
                    child_index = next_index;
                }
                continue;
            }
        }

        if child.kind() != "comment" {
            expressions.push(lower_node_with_rope(child, rope, lowering_context));
        }

        child_index += 1;
    }

    expressions
}

enum AnnotationParseOutcome {
    Annotation(Annotation),
    Definitions(Vec<Definition>),
    MissingTypeExpression,
    Error(TypeParseError),
}

fn attach_annotation_to_expression(
    expression_id: ExpressionId,
    annotation: &Annotation,
    arena: &mut HirArena,
) {
    let expression = arena.get_mut(expression_id);
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

fn annotation_parse_diagnostic(range: Range, error: TypeParseError) -> Diagnostic {
    match error {
        TypeParseError::InvalidSyntax { message } => {
            Diagnostic::syntax_error(range, format!("type syntax error: {message}"))
        }
        TypeParseError::UnsupportedConstruct { message } => {
            Diagnostic::syntax_error(range, format!("unsupported syntax: {message}"))
        }
        TypeParseError::InvalidSemantics { message } => {
            Diagnostic::syntax_error(range, format!("invalid semantics: {message}"))
        }
        TypeParseError::UnknownType { name } => {
            Diagnostic::syntax_error(range, format!("type syntax error: unknown type `{name}`"))
        }
    }
}

fn validate_declared_type_surface_type(
    surface_type: &SurfaceType,
    declared_type_names: &BTreeSet<Symbol>,
    type_parameters: &[Symbol],
    interner: &Interner,
) -> Result<(), TypeParseError> {
    let local_type_parameters = type_parameters.iter().copied().collect::<BTreeSet<_>>();
    validate_surface_type_names(
        surface_type,
        declared_type_names,
        &local_type_parameters,
        interner,
    )
}

fn validate_annotation(
    annotation: &Annotation,
    declared_type_names: &BTreeSet<Symbol>,
    interner: &Interner,
) -> Result<(), TypeParseError> {
    match annotation {
        Annotation::Type { surface_type, .. } => validate_surface_type_names(
            surface_type,
            declared_type_names,
            &BTreeSet::new(),
            interner,
        ),
        Annotation::New { .. } => Ok(()),
    }
}

fn validate_surface_type_names(
    surface_type: &SurfaceType,
    declared_type_names: &BTreeSet<Symbol>,
    local_type_parameters: &BTreeSet<Symbol>,
    interner: &Interner,
) -> Result<(), TypeParseError> {
    match surface_type {
        SurfaceType::Named(name, arguments) => {
            if !declared_type_names.contains(name) && !local_type_parameters.contains(name) {
                let name = interner.resolve(*name).unwrap_or("<unknown>").to_owned();
                return Err(TypeParseError::UnknownType { name });
            }

            for argument in arguments {
                validate_surface_type_names(
                    argument,
                    declared_type_names,
                    local_type_parameters,
                    interner,
                )?;
            }
        }
        SurfaceType::Nullable(inner_type)
        | SurfaceType::Vector(inner_type)
        | SurfaceType::NamedVector(inner_type)
        | SurfaceType::List(inner_type)
        | SurfaceType::NamedList(inner_type) => {
            validate_surface_type_names(
                inner_type,
                declared_type_names,
                local_type_parameters,
                interner,
            )?;
        }
        SurfaceType::Record(fields) => {
            for field in fields {
                validate_surface_type_names(
                    &field.value,
                    declared_type_names,
                    local_type_parameters,
                    interner,
                )?;
            }
        }
        SurfaceType::Tuple(items) => {
            for item in items {
                validate_surface_type_names(
                    item,
                    declared_type_names,
                    local_type_parameters,
                    interner,
                )?;
            }
        }
        SurfaceType::Function(function_type) => {
            for parameter in &function_type.parameters {
                validate_surface_type_names(
                    parameter,
                    declared_type_names,
                    local_type_parameters,
                    interner,
                )?;
            }
            for parameter in &function_type.named_parameters {
                validate_surface_type_names(
                    &parameter.value,
                    declared_type_names,
                    local_type_parameters,
                    interner,
                )?;
            }
            validate_surface_type_names(
                &function_type.return_type,
                declared_type_names,
                local_type_parameters,
                interner,
            )?;
        }
        SurfaceType::Binders(type_parameters, inner_type) => {
            let mut nested_type_parameters = local_type_parameters.clone();
            nested_type_parameters.extend(type_parameters.iter().copied());
            validate_surface_type_names(
                inner_type,
                declared_type_names,
                &nested_type_parameters,
                interner,
            )?;
        }
        SurfaceType::Any | SurfaceType::Unknown | SurfaceType::Null | SurfaceType::Scalar(_) => {}
    }

    Ok(())
}
