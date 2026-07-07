use {
    crate::{
        diagnostic::Diagnostic,
        document::Document,
        hir::{
            Argument, AssignTarget, AssignmentScope, Definition, DefinitionId, DefinitionItem,
            Expression, ExpressionId, ExpressionKind, HirArena, Module, Parameter,
        },
        interner::{Interner, Symbol},
        text,
        tree::{field, kind},
        type_syntax::{TypeParseError, TypeSyntax, parse_type_syntax},
        types::{Annotation, Atomic, AttachedAnnotation},
    },
    ropey::Rope,
    tree_sitter::{Node, Range},
};

#[derive(Debug)]
pub struct LoweringContext {
    arena: HirArena,
    diagnostics: Vec<Diagnostic>,
    strict_override: Option<bool>,
    interner: Interner,
    // Current expression-nesting depth, bounded by `LOWER_RECURSION_LIMIT` so a pathologically nested
    // (but otherwise valid) tree cannot overflow the stack during the recursive lowering walk.
    depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweringResult {
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
}

impl Default for LoweringContext {
    fn default() -> Self {
        Self::new()
    }
}

impl LoweringContext {
    pub fn new() -> Self {
        Self {
            arena: HirArena::new(),
            diagnostics: Vec::new(),
            strict_override: None,
            interner: Interner::new(),
            depth: 0,
        }
    }

    pub fn with_interner(interner: Interner) -> Self {
        Self {
            arena: HirArena::new(),
            diagnostics: Vec::new(),
            strict_override: None,
            interner,
            depth: 0,
        }
    }

    pub fn into_interner(self) -> Interner {
        self.interner
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

    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
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
}

pub fn lower(document: &Document, lowering_context: &mut LoweringContext) -> Module {
    lowering_context.arena = HirArena::new();
    lowering_context.diagnostics.clear();
    lowering_context.strict_override = None;

    let root = document.tree().root_node();
    let rope = document.rope();
    let (definitions, expressions) = lower_module(root, rope, lowering_context);

    let arena = std::mem::take(&mut lowering_context.arena);
    Module::with_strict_override(
        arena,
        definitions,
        expressions,
        lowering_context.strict_override,
    )
}

pub fn lower_with_diagnostics(
    document: &Document,
    lowering_context: &mut LoweringContext,
) -> LoweringResult {
    let root = document.tree().root_node();
    if root.has_error() {
        return LoweringResult {
            module: Module::new(HirArena::new(), Vec::new(), Vec::new()),
            diagnostics: collect_syntax_errors(root, document.rope()),
        };
    }

    let module = lower(document, lowering_context);
    let diagnostics = lowering_context.take_diagnostics();

    LoweringResult {
        module,
        diagnostics,
    }
}

pub(crate) fn lower_with_shared_interner(
    document: &Document,
    interner: &mut Interner,
) -> LoweringResult {
    let mut lowering_context = LoweringContext::with_interner(std::mem::take(interner));
    let lowering_result = lower_with_diagnostics(document, &mut lowering_context);
    *interner = lowering_context.into_interner();
    lowering_result
}

// Recursive-descent depth bound for lowering. The walk recurses once per level of expression
// nesting; a deeply nested but otherwise valid tree (e.g. hundreds of nested `{ }` blocks — which
// carry no syntax error, so lowering proceeds) would otherwise overflow the stack and abort the
// process. This sits well below the measured ~325-level overflow on a 2 MB stack and mirrors the
// type-syntax guard (`TYPE_SYNTAX_RECURSION_LIMIT`); past it, the subtree lowers to `Unsupported`
// with one diagnostic instead of recursing further.
const LOWER_RECURSION_LIMIT: usize = 160;

fn lower_node_with_rope(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionId {
    if lowering_context.depth >= LOWER_RECURSION_LIMIT {
        lowering_context.diagnostics.push(Diagnostic::syntax_error(
            node.range(),
            format!(
                "This expression is nested too deeply to analyze (more than {LOWER_RECURSION_LIMIT} levels)."
            ),
        ));
        return lowering_context.annotated_expression(
            node.range(),
            None,
            ExpressionKind::Unsupported,
        );
    }
    lowering_context.depth += 1;
    let result = lower_node_inner(node, rope, lowering_context);
    lowering_context.depth -= 1;
    result
}

fn lower_node_inner(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionId {
    let kind = match node.kind_id() {
        kind::IDENTIFIER => ExpressionKind::Symbol(intern_node_text(node, rope, lowering_context)),
        kind::NULL => ExpressionKind::Null,
        kind::TRUE => ExpressionKind::Logical(true),
        kind::FALSE => ExpressionKind::Logical(false),
        kind::INTEGER => ExpressionKind::Integer(node_text(node, rope)),
        kind::FLOAT => ExpressionKind::Double(node_text(node, rope)),
        // `Inf` and `NaN` are reserved `double` constants; `1i` is `complex`.
        kind::INF | kind::NAN => ExpressionKind::Double(node_text(node, rope)),
        kind::COMPLEX => ExpressionKind::AtomicConstant(Atomic::Complex),
        // `NA` is logical; the typed `NA_*` forms carry their atomic type.
        kind::NA => ExpressionKind::AtomicConstant(na_atomic(node, rope)),
        kind::STRING => ExpressionKind::Character(node_text(node, rope)),
        kind::BRACED_EXPRESSION => lower_block(node, rope, lowering_context),
        kind::BINARY_OPERATOR => lower_binary_operator(node, rope, lowering_context),
        kind::UNARY_OPERATOR => lower_unary_operator(node, rope, lowering_context),
        kind::FUNCTION_DEFINITION => lower_function_definition(node, rope, lowering_context),
        kind::IF_STATEMENT => lower_if_statement(node, rope, lowering_context),
        kind::FOR_STATEMENT => lower_for_statement(node, rope, lowering_context),
        kind::WHILE_STATEMENT => lower_while_statement(node, rope, lowering_context),
        kind::REPEAT_STATEMENT => lower_repeat_statement(node, rope, lowering_context),
        kind::BREAK => ExpressionKind::Break,
        kind::NEXT => ExpressionKind::Next,
        kind::CALL => lower_call(node, rope, lowering_context),
        kind::SUBSET => lower_subset(node, rope, lowering_context),
        kind::SUBSET2 => lower_subset2(node, rope, lowering_context),
        kind::EXTRACT_OPERATOR => lower_extract_operator(node, rope, lowering_context),
        kind::NAMESPACE_OPERATOR => lower_namespace_operator(node, rope, lowering_context),
        kind::PARENTHESIZED_EXPRESSION => {
            if let Some(inner) = first_named_child(node) {
                return lower_node_with_rope(inner, rope, lowering_context);
            }
            ExpressionKind::Unsupported
        }
        _ => ExpressionKind::Unsupported,
    };

    lowering_context.annotated_expression(node.range(), None, kind)
}

// `pkg::name` / `pkg:::name`. Only the plain identifier-on-both-sides shape is modeled; string
// or computed sides stay unsupported.
fn lower_namespace_operator(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(lhs) = node.child_by_field_id(field::LHS) else {
        return ExpressionKind::Unsupported;
    };
    let Some(rhs) = node.child_by_field_id(field::RHS) else {
        return ExpressionKind::Unsupported;
    };
    if lhs.kind_id() != kind::IDENTIFIER || rhs.kind_id() != kind::IDENTIFIER {
        return ExpressionKind::Unsupported;
    }
    ExpressionKind::NamespaceGet {
        namespace: intern_node_text(lhs, rope, lowering_context),
        name: intern_node_text(rhs, rope, lowering_context),
    }
}

fn lower_binary_operator(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let maybe_lhs = node.child_by_field_id(field::LHS);
    let maybe_operator = node.child_by_field_id(field::OPERATOR);
    let maybe_rhs = node.child_by_field_id(field::RHS);

    let Some(lhs) = maybe_lhs else {
        return ExpressionKind::Unsupported;
    };
    let Some(operator) = maybe_operator else {
        return ExpressionKind::Unsupported;
    };
    let Some(rhs) = maybe_rhs else {
        return ExpressionKind::Unsupported;
    };

    match operator.kind_id() {
        kind::LEFT_ASSIGN
        | kind::EQUAL
        | kind::LEFT_ASSIGN2
        | kind::RIGHT_ASSIGN
        | kind::RIGHT_ASSIGN2 => {
            // Right assignment mirrors left assignment: `value -> name` is `name <- value` and
            // `value ->> name` is `name <<- value`.
            let (target_node, value_node) = match operator.kind_id() {
                kind::RIGHT_ASSIGN | kind::RIGHT_ASSIGN2 => (rhs, lhs),
                _ => (lhs, rhs),
            };
            let scope = match operator.kind_id() {
                kind::LEFT_ASSIGN2 | kind::RIGHT_ASSIGN2 => AssignmentScope::Enclosing,
                _ => AssignmentScope::Local,
            };

            let target = if target_node.kind_id() == kind::IDENTIFIER {
                AssignTarget::Variable {
                    symbol: intern_node_text(target_node, rope, lowering_context),
                    range: target_node.range(),
                }
            } else {
                // Any non-name target is a replacement form (`x[i] <- v`, `names(x) <- v`, ...).
                // The target lowers as an ordinary expression so the base read and every
                // index/argument expression stay visible to naming and typecheck; targets whose
                // accessor spine has no variable at its root are refused there, never silently.
                AssignTarget::Replacement {
                    lhs: lower_node_with_rope(target_node, rope, lowering_context),
                }
            };
            let value = lower_node_with_rope(value_node, rope, lowering_context);

            ExpressionKind::Assign {
                target,
                scope,
                value,
            }
        }
        kind::SPECIAL => {
            let operator_text = node_text(operator, rope);
            if operator_text != "%%" && operator_text != "%/%" {
                return ExpressionKind::Unsupported;
            }

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
        kind::PLUS
        | kind::MINUS
        | kind::STAR
        | kind::SLASH
        | kind::DOUBLE_STAR
        | kind::CARET
        | kind::COLON
        | kind::LT
        | kind::LTE
        | kind::GT
        | kind::GTE
        | kind::EQEQ
        | kind::NEQ
        | kind::DOUBLE_AMPERSAND
        | kind::DOUBLE_PIPE => {
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
    let expressions = lower_block_expressions(node, rope, lowering_context);

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
    let Some(operator) = node.child_by_field_id(field::OPERATOR) else {
        return ExpressionKind::Unsupported;
    };
    let Some(value) = node.child_by_field_id(field::RHS) else {
        return ExpressionKind::Unsupported;
    };

    match operator.kind_id() {
        kind::MINUS => ExpressionKind::UnaryMinus {
            value: lower_node_with_rope(value, rope, lowering_context),
        },
        kind::EXCLAMATION => ExpressionKind::UnaryNot {
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
    let (parameters, variadic) = node
        .child_by_field_id(field::PARAMETERS)
        .map(|parameters| lower_parameters(parameters, rope, lowering_context))
        .unwrap_or_default();

    let body = node
        .child_by_field_id(field::BODY)
        .map(|body| lower_node_with_rope(body, rope, lowering_context))
        .unwrap_or_else(|| lowering_context.expression(node.range(), ExpressionKind::Unsupported));

    ExpressionKind::Function {
        parameters,
        variadic,
        body,
    }
}

fn lower_if_statement(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(condition) = node.child_by_field_id(field::CONDITION) else {
        return ExpressionKind::Unsupported;
    };
    let Some(consequence) = node.child_by_field_id(field::CONSEQUENCE) else {
        return ExpressionKind::Unsupported;
    };

    ExpressionKind::If {
        condition: lower_node_with_rope(condition, rope, lowering_context),
        consequence: lower_node_with_rope(consequence, rope, lowering_context),
        alternative: node
            .child_by_field_id(field::ALTERNATIVE)
            .map(|alternative| lower_node_with_rope(alternative, rope, lowering_context)),
    }
}

fn lower_for_statement(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(variable) = node.child_by_field_id(field::VARIABLE) else {
        return ExpressionKind::Unsupported;
    };
    if variable.kind_id() != kind::IDENTIFIER {
        return ExpressionKind::Unsupported;
    }
    let Some(sequence) = node.child_by_field_id(field::SEQUENCE) else {
        return ExpressionKind::Unsupported;
    };
    let Some(body) = node.child_by_field_id(field::BODY) else {
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
    let Some(condition) = node.child_by_field_id(field::CONDITION) else {
        return ExpressionKind::Unsupported;
    };
    let Some(body) = node.child_by_field_id(field::BODY) else {
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
    let Some(body) = node.child_by_field_id(field::BODY) else {
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
) -> (Vec<Parameter>, Option<usize>) {
    let mut lowered_parameters = Vec::new();
    let mut variadic = None;
    let child_count = parameters.named_child_count();

    for child_index in 0..child_count {
        let Some(child) = parameters.named_child(child_index) else {
            continue;
        };

        match child.kind_id() {
            kind::IDENTIFIER => {
                lowered_parameters.push(Parameter {
                    symbol: intern_node_text(child, rope, lowering_context),
                    range: child.range(),
                    default: None,
                });
            }
            kind::PARAMETER => {
                if let Some(name) = child.child_by_field_id(field::NAME) {
                    match name.kind_id() {
                        kind::IDENTIFIER => {
                            let default = child.child_by_field_id(field::DEFAULT).map(|default| {
                                lower_node_with_rope(default, rope, lowering_context)
                            });
                            lowered_parameters.push(Parameter {
                                symbol: intern_node_text(name, rope, lowering_context),
                                range: name.range(),
                                default,
                            });
                        }
                        // A `...` formal binds no name; only its position matters (formals before
                        // it fill positionally, formals after it by name only). R rejects a second
                        // `...` at parse time, so keeping the first is enough.
                        kind::DOTS if variadic.is_none() => {
                            variadic = Some(lowered_parameters.len());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    (lowered_parameters, variadic)
}

fn lower_call(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(function) = node.child_by_field_id(field::FUNCTION) else {
        return ExpressionKind::Unsupported;
    };

    // R's `local(expr)` is a scope-introducing form, not an ordinary call: it evaluates `expr` in a
    // fresh child environment and returns its value. Recognize the syntactic single-argument call to the
    // bare name `local` and lower it to a `Local` node so naming scopes its body and typecheck takes the
    // body's type. This treats the *syntactic* `local(...)` as the construct; a user who rebinds `local`
    // to their own function would still get this scoping (a v1 caveat, noted in the stdlib stub for
    // `local`), which matches the common intent and is the safe direction (extra scoping, never a leak).
    if let Some(body) = single_local_argument(node, function, rope) {
        let body = lower_node_with_rope(body, rope, lowering_context);
        return ExpressionKind::Local { body };
    }

    // `return(x)` / `return()` is control flow, not a call: modeling it as a call would mistype
    // every early-return function and warn that `return` is unresolved. Like `local`, the
    // syntactic call to the bare name is the construct (rebinding `return` is not modeled). Any
    // other shape (`return(a, b)`, `return(x = 1)`) stays an ordinary call, which R rejects at
    // run time anyway.
    if let Some(value) = return_argument(node, function, rope) {
        let value = value.map(|value| lower_node_with_rope(value, rope, lowering_context));
        return ExpressionKind::Return { value };
    }

    let callee = lower_node_with_rope(function, rope, lowering_context);
    let arguments = node
        .child_by_field_id(field::ARGUMENTS)
        .map(|arguments| lower_arguments(arguments, rope, lowering_context))
        .unwrap_or_default();

    ExpressionKind::Call { callee, arguments }
}

// The body node of a `local(<expr>)` call: `Some` only when `function` is the bare identifier `local`
// and the call has exactly one positional (unnamed) argument carrying a value. Any other shape
// (`local(a, b)`, `local(x = e)`, `local()`, or `pkg::local(e)`) is left as an ordinary call.
fn single_local_argument<'tree>(
    call: Node<'tree>,
    function: Node<'_>,
    rope: &Rope,
) -> Option<Node<'tree>> {
    if function.kind_id() != kind::IDENTIFIER || node_text(function, rope) != "local" {
        return None;
    }
    let arguments = call.child_by_field_id(field::ARGUMENTS)?;
    let mut argument_nodes = (0..arguments.named_child_count())
        .filter_map(|index| arguments.named_child(index))
        .filter(|child| child.kind_id() == kind::ARGUMENT);
    let argument = argument_nodes.next()?;
    if argument_nodes.next().is_some() || argument.child_by_field_id(field::NAME).is_some() {
        return None;
    }
    argument.child_by_field_id(field::VALUE)
}

// The value node of a `return(<expr>)` / `return()` call: `Some` only when `function` is the bare
// identifier `return` with zero or one positional (unnamed) argument (the inner `Option` is the
// value, absent for `return()`). Any other shape is left as an ordinary call.
fn return_argument<'tree>(
    call: Node<'tree>,
    function: Node<'_>,
    rope: &Rope,
) -> Option<Option<Node<'tree>>> {
    if function.kind_id() != kind::IDENTIFIER || node_text(function, rope) != "return" {
        return None;
    }
    let Some(arguments) = call.child_by_field_id(field::ARGUMENTS) else {
        return Some(None);
    };
    let mut argument_nodes = (0..arguments.named_child_count())
        .filter_map(|index| arguments.named_child(index))
        .filter(|child| child.kind_id() == kind::ARGUMENT);
    let Some(argument) = argument_nodes.next() else {
        return Some(None);
    };
    if argument_nodes.next().is_some() || argument.child_by_field_id(field::NAME).is_some() {
        return None;
    }
    Some(argument.child_by_field_id(field::VALUE))
}

fn lower_subset(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(function) = node.child_by_field_id(field::FUNCTION) else {
        return ExpressionKind::Unsupported;
    };

    let value = lower_node_with_rope(function, rope, lowering_context);
    let arguments = node
        .child_by_field_id(field::ARGUMENTS)
        .map(|arguments| lower_index_arguments(arguments, rope, lowering_context))
        .unwrap_or_default();

    ExpressionKind::Subset { value, arguments }
}

fn lower_subset2(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(function) = node.child_by_field_id(field::FUNCTION) else {
        return ExpressionKind::Unsupported;
    };

    let value = lower_node_with_rope(function, rope, lowering_context);
    let arguments = node
        .child_by_field_id(field::ARGUMENTS)
        .map(|arguments| lower_index_arguments(arguments, rope, lowering_context))
        .unwrap_or_default();

    ExpressionKind::Subset2 { value, arguments }
}

fn lower_extract_operator(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> ExpressionKind {
    let Some(operator) = node.child_by_field_id(field::OPERATOR) else {
        return ExpressionKind::Unsupported;
    };
    if operator.kind_id() != kind::DOLLAR && operator.kind_id() != kind::AT {
        return ExpressionKind::Unsupported;
    }

    let Some(lhs) = node.child_by_field_id(field::LHS) else {
        return ExpressionKind::Unsupported;
    };
    let Some(rhs) = node.child_by_field_id(field::RHS) else {
        return ExpressionKind::Unsupported;
    };
    let name = match rhs.kind_id() {
        kind::IDENTIFIER => intern_node_text(rhs, rope, lowering_context),
        kind::STRING => intern_string_node_content(rhs, rope, lowering_context),
        _ => return ExpressionKind::Unsupported,
    };

    let value = lower_node_with_rope(lhs, rope, lowering_context);
    if operator.kind_id() == kind::AT {
        ExpressionKind::Slot { value, name }
    } else {
        ExpressionKind::Dollar { value, name }
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

        if child.kind_id() != kind::ARGUMENT {
            continue;
        }

        let name = child
            .child_by_field_id(field::NAME)
            .filter(|name| name.kind_id() == kind::IDENTIFIER)
            .map(|name| intern_node_text(name, rope, lowering_context));

        let expression = child
            .child_by_field_id(field::VALUE)
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

        if child.kind_id() != kind::ARGUMENT {
            continue;
        }

        let name = child
            .child_by_field_id(field::NAME)
            .filter(|name| name.kind_id() == kind::IDENTIFIER)
            .map(|name| intern_node_text(name, rope, lowering_context));

        let expression = match child.child_by_field_id(field::VALUE) {
            None => lowering_context.expression(child.range(), ExpressionKind::Unsupported),
            Some(value) if value.kind_id() == kind::STRING => {
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

// `NA` is logical; the typed sentinels carry a fixed atomic type.
fn na_atomic(node: Node<'_>, rope: &Rope) -> Atomic {
    match node_text(node, rope).as_str() {
        "NA_integer_" => Atomic::Integer,
        "NA_real_" => Atomic::Double,
        "NA_complex_" => Atomic::Complex,
        "NA_character_" => Atomic::Character,
        _ => Atomic::Logical,
    }
}

fn intern_string_node_content(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> Symbol {
    if let Some(content) = node.child_by_field_id(field::CONTENT) {
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

fn lower_module(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> (Vec<DefinitionItem>, Vec<ExpressionId>) {
    let mut definitions = Vec::new();
    let mut expressions = Vec::new();

    for sequence_item in lower_sequence(node, rope, lowering_context, SequenceContext::Module) {
        match sequence_item {
            LoweredSequenceItem::Definition { range, definition } => {
                let definition_id = DefinitionId(definitions.len() as u32);
                definitions.push(DefinitionItem::new(definition_id, range, definition));
            }
            LoweredSequenceItem::Expression(expression_id) => {
                expressions.push(expression_id);
            }
        }
    }

    (definitions, expressions)
}

fn lower_block_expressions(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
) -> Vec<ExpressionId> {
    let mut expressions = Vec::new();

    for sequence_item in lower_sequence(node, rope, lowering_context, SequenceContext::Block) {
        if let LoweredSequenceItem::Expression(expression_id) = sequence_item {
            expressions.push(expression_id);
        }
    }

    expressions
}

fn lower_sequence(
    node: Node<'_>,
    rope: &Rope,
    lowering_context: &mut LoweringContext,
    sequence_context: SequenceContext,
) -> Vec<LoweredSequenceItem> {
    let mut items = Vec::new();
    let child_count = node.named_child_count();
    let mut child_index = 0;

    while child_index < child_count {
        let Some(child) = node.named_child(child_index) else {
            child_index += 1;
            continue;
        };

        if child.kind_id() == kind::COMMENT {
            let text = node_text(child, rope);
            let trimmed = text.trim_start();
            if trimmed.starts_with("#:") {
                let mut block_range = child.range();
                let mut block_lines = vec![(trimmed.to_string(), child.range())];

                let mut next_index = child_index + 1;
                while next_index < child_count {
                    if let Some(next_child) = node.named_child(next_index)
                        && next_child.kind_id() == kind::COMMENT
                    {
                        let next_text = node_text(next_child, rope);
                        let next_trimmed = next_text.trim_start();
                        if next_trimmed.starts_with("#:") {
                            if next_child.range().start_point.row > block_range.end_point.row + 1 {
                                break;
                            }
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

                // A top-level `#: @strict` / `#: @strict off` block is a per-file switch for
                // the strict check, not type syntax; it overrides the configured default for
                // this file (last directive wins).
                if matches!(sequence_context, SequenceContext::Module) {
                    match stripped_text.trim() {
                        "@strict" | "@strict on" => {
                            lowering_context.strict_override = Some(true);
                            child_index = next_index;
                            continue;
                        }
                        "@strict off" => {
                            lowering_context.strict_override = Some(false);
                            child_index = next_index;
                            continue;
                        }
                        _ => {}
                    }
                }

                let parsed_annotation = if stripped_text.trim().is_empty() {
                    AnnotationParseOutcome::MissingTypeExpression
                } else {
                    match parse_type_syntax(&stripped_text, lowering_context.interner_mut()) {
                        Ok(TypeSyntax::Definitions(definitions)) => {
                            AnnotationParseOutcome::Definitions(definitions)
                        }
                        Ok(TypeSyntax::Annotation(annotation)) => {
                            AnnotationParseOutcome::Annotation(annotation)
                        }
                        Err(error) => AnnotationParseOutcome::Error(error),
                    }
                };

                let parsed_annotation = match parsed_annotation {
                    AnnotationParseOutcome::Definitions(definitions) => {
                        if matches!(sequence_context, SequenceContext::Block) {
                            lowering_context.diagnostics.push(Diagnostic::annotation_error(
                                block_range,
                                "Type definition blocks are only allowed at the top level of a file.",
                            ));
                            child_index = next_index;
                            continue;
                        }

                        for (definition, range) in definitions
                            .into_iter()
                            .zip(block_lines.iter().map(|(_, range)| *range))
                        {
                            let Definition {
                                name,
                                type_parameters,
                                surface_type,
                                kind: definition_kind,
                            } = definition;
                            let definition = Definition {
                                kind: definition_kind,
                                name,
                                type_parameters,
                                surface_type,
                            };
                            items.push(LoweredSequenceItem::Definition { range, definition });
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
                        lowering_context.diagnostics.push(Diagnostic::annotation_error(
                            block_range,
                            "A `#:` typing comment cannot be separated from its expression by an empty line.",
                        ));
                        child_index = next_index;
                    } else if expr_child.kind_id() == kind::COMMENT {
                        lowering_context.diagnostics.push(Diagnostic::annotation_error(
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
                                    block_range,
                                    &mut lowering_context.arena,
                                );
                            }
                            AnnotationParseOutcome::MissingTypeExpression => {
                                lowering_context
                                    .diagnostics
                                    .push(Diagnostic::annotation_error(
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

                        items.push(LoweredSequenceItem::Expression(expr_id));
                        child_index = next_index + 1;
                    }
                } else {
                    lowering_context
                        .diagnostics
                        .push(Diagnostic::annotation_error(
                            block_range,
                            "A `#:` typing comment must be followed immediately by an expression.",
                        ));
                    child_index = next_index;
                }
                continue;
            }
        }

        if child.kind_id() != kind::COMMENT {
            items.push(LoweredSequenceItem::Expression(lower_node_with_rope(
                child,
                rope,
                lowering_context,
            )));
        }

        child_index += 1;
    }

    items
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequenceContext {
    Module,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LoweredSequenceItem {
    Definition {
        range: Range,
        definition: Definition,
    },
    Expression(ExpressionId),
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
    annotation_range: Range,
    arena: &mut HirArena,
) {
    let expression = arena.get_mut(expression_id);
    expression.annotation = Some(match expression.kind {
        ExpressionKind::Assign { .. } => {
            AttachedAnnotation::binding_and_expression(annotation.clone(), annotation_range)
        }
        _ => AttachedAnnotation::expression(annotation.clone(), annotation_range),
    });
}

fn annotation_parse_diagnostic(range: Range, error: TypeParseError) -> Diagnostic {
    match error {
        TypeParseError::InvalidSyntax { message } => {
            Diagnostic::annotation_error(range, format!("type syntax error: {message}"))
        }
        TypeParseError::UnsupportedConstruct { message } => {
            Diagnostic::annotation_error(range, format!("unsupported syntax: {message}"))
        }
        TypeParseError::InvalidSemantics { message } => {
            Diagnostic::annotation_error(range, format!("invalid semantics: {message}"))
        }
        TypeParseError::UnknownType { name } => {
            Diagnostic::annotation_error(range, format!("type syntax error: unknown type `{name}`"))
        }
        TypeParseError::RecursionLimitExceeded { limit } => Diagnostic::annotation_error(
            range,
            format!(
                "This type annotation is nested too deeply to parse (more than {limit} levels)."
            ),
        ),
    }
}

// The syntax-error walk over a malformed tree. Iterative with an explicit frame stack: tree-sitter's
// error recovery nests ERROR nodes as deeply as the source nests, and unlike lowering (which caps its
// depth and degrades to `Unsupported`) this walk must reach every error node to report it, so a
// recursive formulation would overflow the stack on deeply nested malformed input.
//
// Each node visit has three parts, mirrored from the natural recursion: pre-order per-kind checks
// (`enter_syntax_error_node`), a fold of the children's "handled" verdicts, and a post-order
// fallback that reports the raw error text only when no descendant produced a more precise message.
fn collect_syntax_errors(root: Node<'_>, rope: &Rope) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut stack = match enter_syntax_error_node(root, rope, &mut diagnostics) {
        Entered::Done(_) => return diagnostics,
        Entered::Descend(frame) => vec![frame],
    };
    // The verdict of the most recently completed child, folded into its parent's frame at the top
    // of each iteration.
    let mut returned = false;
    while let Some(frame) = stack.last_mut() {
        frame.handled |= returned;
        returned = false;
        match frame.children.next() {
            Some(child) => match enter_syntax_error_node(child, rope, &mut diagnostics) {
                Entered::Done(handled) => returned = handled,
                Entered::Descend(child_frame) => stack.push(child_frame),
            },
            None => {
                let frame = stack.pop().expect("frame observed by last_mut");
                let mut handled = frame.handled;
                if !handled && frame.node.is_error() {
                    handled = true;
                    let raw = rope.byte_slice(frame.node.byte_range()).to_string();
                    match raw.as_str() {
                        "(" | "{" | "[" | "[[" => diagnostics.push(Diagnostic::syntax_error(
                            frame.node.range(),
                            format!("unexpected opening delimiter {raw}"),
                        )),
                        ")" | "}" | "]" | "]]" => diagnostics.push(Diagnostic::syntax_error(
                            frame.node.range(),
                            format!("unexpected closing delimiter {raw}"),
                        )),
                        _ => diagnostics.push(Diagnostic::syntax_error(
                            frame.node.range(),
                            format!("Syntax Error: unexpected {raw:?}"),
                        )),
                    }
                }
                returned = handled;
            }
        }
    }
    diagnostics
}

struct SyntaxErrorFrame<'tree> {
    node: Node<'tree>,
    children: std::vec::IntoIter<Node<'tree>>,
    handled: bool,
}

enum Entered<'tree> {
    // The subtree finished during the pre-order half: it is clean, or a recognized control-flow
    // head consumed it. Carries the subtree's "handled" verdict.
    Done(bool),
    Descend(SyntaxErrorFrame<'tree>),
}

fn enter_syntax_error_node<'tree>(
    node: Node<'tree>,
    rope: &Rope,
    diagnostics: &mut Vec<Diagnostic>,
) -> Entered<'tree> {
    if !(node.is_error() || node.has_error()) {
        return Entered::Done(false);
    }

    match node.kind_id() {
        kind::ARGUMENTS
        | kind::BRACED_EXPRESSION
        | kind::PARAMETERS
        | kind::PARENTHESIZED_EXPRESSION => {
            if let Some(open) = node.child_by_field_id(field::OPEN)
                && let Some(close) = node.child_by_field_id(field::CLOSE)
                && close.is_missing()
            {
                diagnostics.push(Diagnostic::syntax_error(
                    open.range(),
                    format!("missing closing delimiter {}", close.kind()),
                ));
            }
        }
        kind::BINARY_OPERATOR => {
            if let Some(operator) = node.child_by_field_id(field::OPERATOR)
                && let Some(right_hand_side) = node.child_by_field_id(field::RHS)
                && right_hand_side.is_missing()
            {
                diagnostics.push(Diagnostic::syntax_error(
                    operator.range(),
                    format!("missing rhs for operator {}", operator.kind()),
                ));
            }
        }
        kind::FUNCTION_DEFINITION => {
            if let Some(body) = node.child_by_field_id(field::BODY)
                && body.is_missing()
            {
                diagnostics.push(Diagnostic::syntax_error(
                    node.range(),
                    "missing function body",
                ));
            }
        }
        kind::IF_STATEMENT => {
            if let Some(consequence) = node.child_by_field_id(field::CONSEQUENCE)
                && consequence.is_missing()
            {
                diagnostics.push(Diagnostic::syntax_error(node.range(), "missing if body"));
            }
        }
        kind::FOR_STATEMENT => {
            if let Some(body) = node.child_by_field_id(field::BODY)
                && body.is_missing()
            {
                diagnostics.push(Diagnostic::syntax_error(node.range(), "missing for body"));
            }
        }
        kind::WHILE_STATEMENT => {
            if let Some(body) = node.child_by_field_id(field::BODY)
                && body.is_missing()
            {
                diagnostics.push(Diagnostic::syntax_error(node.range(), "missing while body"));
            }
        }
        _ => {}
    }

    if node.is_error()
        && let Some((range, message)) = control_flow_head_syntax_error(node, rope)
    {
        diagnostics.push(Diagnostic::syntax_error(range, message));
        return Entered::Done(true);
    }

    let mut tree_cursor = node.walk();
    let children: Vec<Node<'tree>> = node.children(&mut tree_cursor).collect();
    if node.is_error()
        && let Some(child) = children.first()
    {
        match child.kind_id() {
            kind::LPAREN | kind::LBRACE | kind::LBRACKET | kind::DOUBLE_LBRACKET => {
                diagnostics.push(Diagnostic::syntax_error(
                    child.range(),
                    format!("missing closing delimiter {}", child.kind()),
                ));
            }
            _ => {}
        }
    }
    Entered::Descend(SyntaxErrorFrame {
        node,
        children: children.into_iter(),
        handled: false,
    })
}

fn control_flow_head_syntax_error(node: Node<'_>, rope: &Rope) -> Option<(Range, &'static str)> {
    let mut tree_cursor = node.walk();
    let mut children = node.children(&mut tree_cursor);
    let keyword = children.next()?;
    let open = children.next()?;

    match keyword.kind_id() {
        kind::FOR | kind::IF | kind::WHILE => {}
        _ => return None,
    }

    if open.kind_id() != kind::LPAREN {
        return None;
    }

    let head_text = rope
        .byte_slice(open.end_byte()..node.byte_range().end)
        .to_string();
    let head_line_text = head_text.lines().next().unwrap_or("");

    if !head_line_text.contains(')') {
        return Some((open.range(), "missing closing delimiter )"));
    }

    match keyword.kind_id() {
        kind::IF | kind::WHILE if head_line_text.trim_start().starts_with(')') => {
            Some((open.range(), "missing condition"))
        }
        kind::FOR => {
            let head_prefix = head_line_text
                .split_once(')')
                .map(|(before_close, _)| before_close.trim_end())?;
            head_prefix
                .ends_with(" in")
                .then_some((open.range(), "missing sequence in for statement"))
        }
        _ => None,
    }
}
