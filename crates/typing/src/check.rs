use {
    crate::{
        diagnostics::Diagnostic,
        infer::{BuiltinKind, InferenceState},
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
    let plus_symbol = lowering_context.intern("+");
    let minus_symbol = lowering_context.intern("-");
    let multiply_symbol = lowering_context.intern("*");
    let divide_symbol = lowering_context.intern("/");
    let power_symbol = lowering_context.intern("**");
    let and_symbol = lowering_context.intern("&&");
    let or_symbol = lowering_context.intern("||");
    let combine_symbol = lowering_context.intern("c");
    let list_symbol = lowering_context.intern("list");
    inference_state.bind_builtin(plus_symbol, BuiltinKind::Plus);
    inference_state.bind_builtin(minus_symbol, BuiltinKind::Minus);
    inference_state.bind_builtin(multiply_symbol, BuiltinKind::Multiply);
    inference_state.bind_builtin(divide_symbol, BuiltinKind::Divide);
    inference_state.bind_builtin(power_symbol, BuiltinKind::Power);
    inference_state.bind_builtin(and_symbol, BuiltinKind::And);
    inference_state.bind_builtin(or_symbol, BuiltinKind::Or);
    inference_state.bind_builtin(combine_symbol, BuiltinKind::Combine);
    inference_state.bind_builtin(list_symbol, BuiltinKind::List);
    if let Err(error) = inference_state.infer_module(&module) {
        diagnostics.push(Diagnostic::from_inference_error(
            &error,
            fallback_range(source),
            lowering_context.interner(),
        ));
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
