use {
    crate::{config::ExperimentalFeatures, server::Document, tree},
    tree_sitter::Node,
};

pub fn markdown(
    document: &Document,
    node: Node,
    experimental_features: ExperimentalFeatures,
) -> Option<String> {
    if is_literal_kind(node.kind_id()) {
        return Some(literal_markdown(
            document,
            normalize_literal_node(node),
            experimental_features.debug,
        ));
    }

    match node.kind_id() {
        tree::kind::IDENTIFIER => Some(identifier_markdown(
            document,
            node,
            experimental_features.debug,
        )),
        tree::kind::IF | tree::kind::IF_STATEMENT => Some(keyword_markdown(
            node,
            "if",
            "Conditional branch.",
            experimental_features.debug,
        )),
        tree::kind::FOR | tree::kind::FOR_STATEMENT => Some(keyword_markdown(
            node,
            "for",
            "Loop over a sequence.",
            experimental_features.debug,
        )),
        tree::kind::WHILE | tree::kind::WHILE_STATEMENT => Some(keyword_markdown(
            node,
            "while",
            "Loop while a condition stays true.",
            experimental_features.debug,
        )),
        tree::kind::REPEAT | tree::kind::REPEAT_STATEMENT => Some(keyword_markdown(
            node,
            "repeat",
            "Loop until interrupted with `break`.",
            experimental_features.debug,
        )),
        tree::kind::FUNCTION | tree::kind::FUNCTION_DEFINITION => Some(keyword_markdown(
            node,
            "function",
            "Function definition.",
            experimental_features.debug,
        )),
        tree::kind::RETURN => Some(keyword_markdown(
            node,
            "return",
            "Return from the current function.",
            experimental_features.debug,
        )),
        tree::kind::BREAK => Some(keyword_markdown(
            node,
            "break",
            "Exit the current loop.",
            experimental_features.debug,
        )),
        tree::kind::NEXT => Some(keyword_markdown(
            node,
            "next",
            "Skip to the next loop iteration.",
            experimental_features.debug,
        )),
        tree::kind::ELSE => Some(keyword_markdown(
            node,
            "else",
            "Alternative branch for an `if` statement.",
            experimental_features.debug,
        )),
        _ => None,
    }
}

fn identifier_markdown(document: &Document, node: Node, debug: bool) -> String {
    let name = document.rope.byte_slice(node.byte_range()).to_string();
    let mut markup = fenced_block("text", &name);

    if debug {
        markup.push_str(&debug_section(node));
    }

    markup
}

fn keyword_markdown(node: Node, keyword: &str, description: &str, debug: bool) -> String {
    let mut markup = format!("{}\n\n---\n\n{}", fenced_block("r", keyword), description);

    if debug {
        markup.push_str(&debug_section(node));
    }

    markup
}

fn literal_markdown(document: &Document, node: Node, debug: bool) -> String {
    let literal_source = document
        .rope
        .byte_slice(node.byte_range())
        .to_string()
        .chars()
        .take_while(|character| *character != '\n')
        .collect::<String>();

    let literal_value = literal_value(document, node);

    let literal_truncated = literal_value
        .chars()
        .take_while(|character| *character != '\n' && *character != '\\')
        .collect::<String>();

    let mut markup = format!(
        "{}\n\n---\n\nvalue of literal{}: ` {} `",
        fenced_block("r", &literal_source),
        if literal_truncated.len() != literal_source.len() {
            "(truncated up to newline):"
        } else {
            ""
        },
        literal_truncated,
    );

    if debug {
        markup.push_str(&debug_section(node));
    }

    markup
}

fn literal_value(document: &Document, node: Node) -> String {
    match node.kind_id() {
        tree::kind::STRING => node
            .child_by_field_id(tree::field::CONTENT)
            .map(|content| document.rope.byte_slice(content.byte_range()).to_string())
            .unwrap_or_default(),
        tree::kind::INTEGER => {
            let integer_text = document
                .rope
                .byte_slice(node.byte_range())
                .to_string()
                .replace('_', "");
            match integer_text.parse::<u128>() {
                Ok(integer) => format!("{integer} (0x{integer:X})"),
                Err(_) => integer_text,
            }
        }
        tree::kind::FLOAT | tree::kind::COMPLEX => document
            .rope
            .byte_slice(node.byte_range())
            .to_string()
            .replace('_', ""),
        _ => document.rope.byte_slice(node.byte_range()).to_string(),
    }
}

fn normalize_literal_node(node: Node) -> Node {
    match node.kind_id() {
        tree::kind::STRING_CONTENT | tree::kind::SINGLE_QUOTE | tree::kind::DOUBLE_QUOTE => {
            node.parent().unwrap_or(node)
        }
        _ => node,
    }
}

fn debug_section(node: Node) -> String {
    format!(
        "\n\n---\n\n### Debug\n\n- kind: `{}`\n- id: `{}`",
        node.kind(),
        node.id()
    )
}

fn is_literal_kind(kind_id: u16) -> bool {
    matches!(
        kind_id,
        tree::kind::TRUE
            | tree::kind::FALSE
            | tree::kind::NULL
            | tree::kind::INF
            | tree::kind::NAN
            | tree::kind::INTEGER
            | tree::kind::COMPLEX
            | tree::kind::FLOAT
            | tree::kind::STRING
            | tree::kind::NA
            | tree::kind::NA_LITERAL
            | tree::kind::NA_INTEGER
            | tree::kind::NA_REAL
            | tree::kind::NA_COMPLEX
            | tree::kind::NA_CHARACTER
            | tree::kind::STRING_CONTENT
            | tree::kind::SINGLE_QUOTE
            | tree::kind::DOUBLE_QUOTE
    )
}

fn fenced_block(language: &str, contents: &str) -> String {
    format!("```{language}\n{contents}\n```")
}
