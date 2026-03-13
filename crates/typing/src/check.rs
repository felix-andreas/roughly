use {
    crate::{
        diagnostics::Diagnostic,
        infer::{InferenceError, InferenceState},
        lower::LoweringContext,
        parse::{new_parser, parse},
    },
    tree_sitter::Node,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub diagnostics: Vec<Diagnostic>,
}

impl CheckResult {
    pub fn render(&self, source: &str) -> String {
        if self.diagnostics.is_empty() {
            return "No diagnostics.\n".to_owned();
        }

        let mut rendered = String::new();

        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index > 0 {
                rendered.push('\n');
            }
            rendered.push_str(&diagnostic.render(source));
        }

        rendered
    }
}

pub fn check(source: &str) -> CheckResult {
    let mut parser = new_parser();
    let tree = parse(&mut parser, source, None);
    let root = tree.root_node();

    let mut diagnostics = Vec::new();

    if root.has_error() {
        collect_syntax_errors(root, source, &mut diagnostics);
        return CheckResult { diagnostics };
    }

    let mut lowering_context = LoweringContext::new();
    let module = lowering_context.lower_tree(&tree, source);

    let mut inference_state = InferenceState::new();
    if let Err(error) = inference_state.infer_module(&module) {
        diagnostics.push(inference_error_to_diagnostic(error, source));
    }

    CheckResult { diagnostics }
}

fn collect_syntax_errors(node: Node<'_>, source: &str, diagnostics: &mut Vec<Diagnostic>) {
    if node.is_error() {
        diagnostics.push(Diagnostic::syntax_error(
            node.range(),
            format!("Unexpected syntax: {}", snippet(node, source)),
        ));
        return;
    }

    if node.is_missing() {
        diagnostics.push(Diagnostic::syntax_error(
            node.range(),
            format!(
                "Missing syntax near {}",
                point_label(node.range().start_point)
            ),
        ));
        return;
    }

    let child_count = node.child_count();
    for child_index in 0..child_count {
        if let Some(child) = node.child(child_index) {
            collect_syntax_errors(child, source, diagnostics);
        }
    }
}

fn snippet(node: Node<'_>, source: &str) -> String {
    let text = node.utf8_text(source.as_bytes()).unwrap_or("<unavailable>");
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if compact.is_empty() {
        "<empty>".to_owned()
    } else if compact.len() > 40 {
        format!("{:?}…", &compact[..40])
    } else {
        format!("{compact:?}")
    }
}

fn point_label(point: tree_sitter::Point) -> String {
    format!("{}:{}", point.row + 1, point.column + 1)
}

fn inference_error_to_diagnostic(error: InferenceError, source: &str) -> Diagnostic {
    let range = fallback_range(source);

    match error {
        InferenceError::UnknownName(_) => Diagnostic::type_error(
            range,
            "Unknown name. This value is used before it has a known type.",
        ),
        InferenceError::ExpectedFunction(actual_type) => Diagnostic::type_error(
            range,
            format!("Expected a function here, but found {actual_type:?}."),
        ),
        InferenceError::OccursCheckFailed { in_type, .. } => Diagnostic::type_error(
            range,
            format!("Recursive type detected while unifying with {in_type:?}."),
        ),
        InferenceError::TypeMismatch { expected, actual } => Diagnostic::type_error(
            range,
            format!("Type mismatch. Expected {expected:?}, but found {actual:?}."),
        ),
        InferenceError::TupleLengthMismatch { expected, actual } => Diagnostic::type_error(
            range,
            format!("Tuple length mismatch. Expected {expected} item(s), but found {actual}."),
        ),
        InferenceError::RecordFieldMismatch {
            expected_fields,
            actual_fields,
        } => Diagnostic::type_error(
            range,
            format!(
                "Record field mismatch. Expected fields {expected_fields:?}, but found {actual_fields:?}."
            ),
        ),
        InferenceError::FunctionArityMismatch { expected, actual } => Diagnostic::type_error(
            range,
            format!(
                "Function arity mismatch. Expected {expected} argument(s), but found {actual}."
            ),
        ),
        InferenceError::NamedParameterMismatch {
            expected_parameters,
            actual_parameters,
        } => Diagnostic::type_error(
            range,
            format!(
                "Named parameter mismatch. Expected parameters {expected_parameters:?}, but found {actual_parameters:?}."
            ),
        ),
        InferenceError::UnknownInferenceVariable(variable) => Diagnostic::type_error(
            range,
            format!("Internal inference error: unknown inference variable {variable:?}."),
        ),
    }
}

fn fallback_range(source: &str) -> tree_sitter::Range {
    let line = source.lines().next().unwrap_or("");
    tree_sitter::Range {
        start_byte: 0,
        end_byte: line.len(),
        start_point: tree_sitter::Point { row: 0, column: 0 },
        end_point: tree_sitter::Point {
            row: 0,
            column: line.len(),
        },
    }
}
