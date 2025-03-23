use {
    crate::{diagnostics::error, lsp_types::Diagnostic},
    ropey::Rope,
    tree_sitter::{Node, TreeCursor},
};

pub fn analyze(node: Node, rope: &Rope) -> Vec<Diagnostic> {
    fn traverse(cursor: &mut TreeCursor, diagnostics: &mut Vec<Diagnostic>, rope: &Rope) -> bool {
        let node = cursor.node();
        if !(node.is_error() || node.has_error()) {
            return false;
        }

        match node.kind() {
            "arguments"
            | "braced_expression"
            | "for_statement"
            | "if_statement"
            | "parameters"
            | "parenthesized_expression"
            | "while_statement" => {
                if let Some(open) = node.child_by_field_name("open") {
                    if let Some(close) = node.child_by_field_name("close") {
                        if close.is_missing() {
                            diagnostics.push(error(
                                open,
                                format!("missing closing delimiter {}", close.kind()),
                            ));
                        }
                    }
                }
            }
            "binary_operator" => {
                if let Some(operator) = node.child_by_field_name("operator") {
                    if let Some(rhs) = node.child_by_field_name("rhs") {
                        if rhs.is_missing() {
                            diagnostics.push(error(
                                operator,
                                format!("missing rhs for operator {}", operator.kind()),
                            ));
                        }
                    }
                }
            }
            "function_definition" => {
                if let Some(body) = node.child_by_field_name("body") {
                    if body.is_missing() {
                        diagnostics.push(error(node, "missing function body".into()));
                    }
                }
            }
            _ => {}
        }

        let mut handled_error = false;
        if cursor.goto_first_child() {
            if node.is_error() {
                let child = cursor.node();
                match child.kind() {
                    "(" | "{" | "[" | "[[" => diagnostics.push(error(
                        child,
                        format!("missing closing delimiter {}", child.kind()),
                    )),
                    _ => {}
                }
            }

            loop {
                handled_error |= traverse(cursor, diagnostics, rope);

                if !cursor.goto_next_sibling() {
                    cursor.goto_parent();
                    break;
                }
            }
        }

        if !handled_error && node.is_error() {
            handled_error = true;
            let raw = rope.byte_slice(node.byte_range()).to_string();
            match raw.as_str() {
                ")" | "}" | "]" | "]]" => diagnostics.push(error(
                    node,
                    format!("Syntax Error: unexpected closing delimiter {}", raw),
                )),
                _ => {
                    diagnostics.push(error(node, format!("Syntax Error: unexpected {:?}", raw)));
                }
            }
        }

        handled_error
    }

    let mut diagnostics = Vec::new();
    let mut cursor = node.walk();
    traverse(&mut cursor, &mut diagnostics, rope);
    diagnostics
}
