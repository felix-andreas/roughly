use tree_sitter::{Node, Tree, TreeCursor};

// todo: consider resusing global parser (maybe behind Mutex??)
pub fn parse(text: impl AsRef<[u8]>, maybe_tree: Option<&Tree>) -> Tree {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_r::LANGUAGE;
    parser
        .set_language(&language.into())
        .expect("Error loading R parser");
    parser.parse(text, maybe_tree).unwrap()
}

#[inline]
pub fn for_each_child<'a, E>(
    cursor: &mut TreeCursor<'a>,
    mut func: impl FnMut(usize, Node<'a>, Option<&'static str>) -> Result<(), E>,
) -> Result<(), E> {
    // foo

    let mut i = 0;
    if cursor.goto_first_child() {
        loop {
            func(i, cursor.node(), cursor.field_name())?;
            if !cursor.goto_next_sibling() {
                cursor.goto_parent();
                break;
            }
            i += 1;
        }
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use tree_sitter::Language;

    #[test]
    fn kind_ids() {
        let kinds = [
            "argument",
            "arguments",
            "binary_operator",
            "braced_expression",
            "call",
            "complex",
            "extract_operator",
            "float",
            "for_statement",
            "function_definition",
            "if_statement",
            "integer",
            "na",
            "namespace_operator",
            "parameter",
            "parameters",
            "parenthesized_expression",
            "program",
            "repeat_statement",
            "string",
            "string_content",
            "subset",
            "subset2",
            "unary_operator",
            "while_statement",
            "!",
            "!=",
            "\"",
            "$",
            "&",
            "&&",
            "'",
            "(",
            ")",
            "*",
            "**",
            "+",
            "-",
            "->",
            "->>",
            "/",
            ":",
            "::",
            ":::",
            ":=",
            "<",
            "<-",
            "<<-",
            "<=",
            "=",
            "==",
            ">",
            ">=",
            "?",
            "@",
            "L",
            "NA",
            "NA_character_",
            "NA_complex_",
            "NA_integer_",
            "NA_real_",
            "[",
            "[[",
            "\\",
            "]",
            "]]",
            "^",
            "break",
            "comma",
            "comment",
            "dot_dot_i",
            "dots",
            "else",
            "escape_sequence",
            "false",
            "for",
            "function",
            "i",
            "identifier",
            "if",
            "in",
            "inf",
            "nan",
            "next",
            "null",
            "repeat",
            "return",
            "special",
            "true",
            "while",
            "{",
            "|",
            "|>",
            "||",
            "}",
            "~",
        ];

        let language: Language = tree_sitter_r::LANGUAGE.into();
        for kind in kinds {
            println!("{kind}: {}", language.id_for_node_kind(kind, false));
            println!("{kind}: {}", language.id_for_node_kind(kind, true));
        }
    }
}
