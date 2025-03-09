use {
    crate::tree,
    std::path::Path,
    tree_sitter::{Node, TreeCursor},
};

pub fn tree(path: &Path) -> Result<(), std::io::Error> {
    let text = std::fs::read_to_string(path).unwrap();
    let tree = tree::parse(&text, None);
    println!("{}", format_tree(tree.root_node()));
    Ok(())
}

pub fn format_tree(node: Node) -> String {
    fn traverse(cursor: &mut TreeCursor, output: &mut String) {
        let indent = "    ".repeat(cursor.depth() as usize);
        let node = cursor.node();

        let start = node.start_position();
        let end = node.end_position();

        output.push_str(&console::style(node.kind()).bold().to_string());
        output.push_str(
            &console::style(format!(
                " {}:{}..{}:{}",
                start.row, start.column, end.row, end.column
            ))
            .italic()
            .to_string(),
        );

        if node.is_missing() {
            output.push_str(" MISSING");
        } else if node.is_error() && node.kind() != "ERROR" {
            output.push_str(" ERROR");
        }

        if cursor.goto_first_child() {
            loop {
                output.push('\n');
                output.push_str(&indent);
                output.push_str("    ");

                if let Some(field_name) = cursor.field_name() {
                    output.push_str(&console::style(field_name).underlined().to_string());
                    output.push_str(": ");
                }

                traverse(cursor, output);

                if !cursor.goto_next_sibling() {
                    cursor.goto_parent();
                    break;
                }
            }
        }
    }

    let mut result = String::new();
    traverse(&mut node.walk(), &mut result);
    result
}
