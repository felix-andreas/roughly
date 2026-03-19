use {
    crate::{
        diagnostics::Diagnostic,
        infer::{BuiltinKind, InferenceState},
        lower::LoweringContext,
        parse::{new_parser, parse},
        surface_types::{TypeParseError, parse_annotation},
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

    collect_annotation_diagnostics(source, &mut diagnostics);

    if !diagnostics.is_empty() {
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

fn collect_annotation_diagnostics(source: &str, diagnostics: &mut Vec<Diagnostic>) {
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
        let annotation_range = tree_sitter::Range {
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
        };

        if annotation_text.is_empty() {
            diagnostics.push(Diagnostic::syntax_error(
                annotation_range,
                "A `#:` typing comment must include a type expression.",
            ));
            row += 1;
            continue;
        }

        let annotation_is_expanded = is_expanded_function_annotation_line(annotation_text);
        let next_row = row + 1;
        if next_row >= lines.len() {
            diagnostics.push(Diagnostic::syntax_error(
                annotation_range,
                "A `#:` typing comment must be followed immediately by an expression.",
            ));
            row += 1;
            continue;
        }

        let next_line = lines[next_row];
        let next_trimmed = next_line.trim_start();

        if next_trimmed.is_empty() {
            diagnostics.push(Diagnostic::syntax_error(
                annotation_range,
                "A `#:` typing comment cannot be separated from its expression by an empty line.",
            ));
            row += 1;
            continue;
        }

        if let Some(next_annotation_text) = next_trimmed.strip_prefix("#:") {
            let next_annotation_text = next_annotation_text.trim();
            let next_annotation_is_expanded =
                is_expanded_function_annotation_line(next_annotation_text);

            if !annotation_is_expanded
                || !next_annotation_is_expanded
                || next_annotation_text.is_empty()
            {
                diagnostics.push(Diagnostic::syntax_error(
                    annotation_range,
                    "A `#:` typing comment must be followed by an expression, not another `#:` typing comment.",
                ));

                row += 1;
                while row < lines.len() {
                    let skipped_line = lines[row];
                    let skipped_trimmed_line = skipped_line.trim_start();
                    let Some(skipped_annotation_text) = skipped_trimmed_line.strip_prefix("#:")
                    else {
                        break;
                    };

                    if !is_expanded_function_annotation_line(skipped_annotation_text.trim()) {
                        break;
                    }

                    row += 1;
                }

                continue;
            }
        }

        let mut interner = crate::Interner::new();
        match parse_annotation(&mut interner, annotation_text) {
            Ok(_annotation) => {}
            Err(TypeParseError::InvalidSyntax) => {
                diagnostics.push(Diagnostic::syntax_error(
                    annotation_range,
                    "Invalid type syntax in `#:` typing comment.",
                ));
            }
            Err(TypeParseError::UnknownType { name }) => {
                diagnostics.push(Diagnostic::type_error(
                    annotation_range,
                    format!("I could not resolve type `{name}`."),
                ));
            }
        }

        row += 1;
    }
}

fn is_expanded_function_annotation_line(text: &str) -> bool {
    text.starts_with("@param ") || text.starts_with("@return ") || text.starts_with("@returns ")
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
