//! Recognition of R's S4 constructor calls (`setClass`, `setGeneric`, `setMethod`, `new`).
//!
//! S4 class/generic/method names are written as string literals inside these calls, so they are
//! invisible to the naming analysis and must be recovered structurally from the tree. Both the symbol
//! outline (document/workspace symbols) and the S4 navigation features (goto / references / rename)
//! need the same call-shape recognition, so it lives here as the single source of truth for *how* an
//! S4 call is shaped. Each consumer builds its own output (outline items vs. occurrences) from these
//! spans; this module only recognizes the call and yields the argument nodes.
//!
//! Everything here returns borrowed tree nodes rather than owned strings, so it stays allocation-light
//! for the hot goto/references/rename path.

use {
    crate::tree::{children_by_field, field, kind},
    ropey::Rope,
    tree_sitter::Node,
};

/// The S4 constructor a call invokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S4Constructor {
    /// `setClass("Name", ...)` — declares a class.
    SetClass,
    /// `setGeneric("name", ...)` — declares a generic.
    SetGeneric,
    /// `setMethod("generic", signature, ...)` — defines a method for a generic on a class signature.
    SetMethod,
    /// `new("Class", ...)` — constructs an instance (a class reference, not a declaration).
    New,
}

/// Recognizes the S4 constructor a `call` node invokes, resolving the callee whether written bare
/// (`setClass`) or namespace-qualified (`methods::setClass`). Returns `None` for any other call.
pub fn s4_constructor(call: Node<'_>, rope: &Rope) -> Option<S4Constructor> {
    match call_function_name(call, rope)?.as_str() {
        "setClass" => Some(S4Constructor::SetClass),
        "setGeneric" => Some(S4Constructor::SetGeneric),
        "setMethod" => Some(S4Constructor::SetMethod),
        "new" => Some(S4Constructor::New),
        _ => None,
    }
}

/// The bare name of a call's callee: the identifier for `f(...)`, or the right-hand side for a
/// namespace-qualified `pkg::f(...)`. Returns `None` for any other callee shape.
pub fn call_function_name(call: Node<'_>, rope: &Rope) -> Option<String> {
    let function = call.child_by_field_id(field::FUNCTION)?;
    match function.kind_id() {
        kind::IDENTIFIER => Some(rope.byte_slice(function.byte_range()).to_string()),
        kind::NAMESPACE_OPERATOR => {
            let rhs = function.child_by_field_id(field::RHS)?;
            (rhs.kind_id() == kind::IDENTIFIER)
                .then(|| rope.byte_slice(rhs.byte_range()).to_string())
        }
        _ => None,
    }
}

/// Resolves a call argument's value by name, falling back to the positional slot at `index` when it is
/// unnamed. This is R's own argument-matching shape (named arguments first, then positional).
pub fn call_argument<'tree>(
    arguments: Node<'tree>,
    rope: &Rope,
    name: &str,
    index: usize,
) -> Option<Node<'tree>> {
    let mut cursor = arguments.walk();
    for argument in children_by_field(arguments, field::ARGUMENT, &mut cursor) {
        if let Some(argument_name) = argument.child_by_field_id(field::NAME)
            && rope.byte_slice(argument_name.byte_range()) == name
        {
            return argument.child_by_field_id(field::VALUE);
        }
    }

    children_by_field(arguments, field::ARGUMENT, &mut arguments.walk())
        .nth(index)
        .filter(|argument| argument.child_by_field_id(field::NAME).is_none())
        .and_then(|argument| argument.child_by_field_id(field::VALUE))
}

/// Like [`call_argument`], but only yields the argument when its value is a string literal.
pub fn string_argument<'tree>(
    arguments: Node<'tree>,
    rope: &Rope,
    name: &str,
    index: usize,
) -> Option<Node<'tree>> {
    let argument = call_argument(arguments, rope, name, index)?;
    (argument.kind_id() == kind::STRING).then_some(argument)
}

/// The `content` node (the text between the quotes) of a string-literal node, if present.
pub fn string_content(string_node: Node<'_>) -> Option<Node<'_>> {
    string_node.child_by_field_id(field::CONTENT)
}

/// The class-name string nodes in a `setMethod` signature: either the single string `"Class"`, or the
/// string elements of a `c("Class", "Other")` vector.
pub fn signature_class_strings<'tree>(signature: Node<'tree>) -> Vec<Node<'tree>> {
    if signature.kind_id() == kind::STRING {
        return vec![signature];
    }
    if signature.kind_id() == kind::CALL
        && let Some(arguments) = signature.child_by_field_id(field::ARGUMENTS)
    {
        let mut cursor = arguments.walk();
        return children_by_field(arguments, field::ARGUMENT, &mut cursor)
            .filter_map(|argument| argument.child_by_field_id(field::VALUE))
            .filter(|value| value.kind_id() == kind::STRING)
            .collect();
    }
    Vec::new()
}
