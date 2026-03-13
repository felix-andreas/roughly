use {
    crate::{
        diagnostics::Diagnostic,
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
