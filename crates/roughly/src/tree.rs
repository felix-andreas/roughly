use {
    ropey::Rope,
    tree_sitter::{Node, Parser, Tree, TreeCursor},
};

pub fn new_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .expect("Error loading R parser");
    parser
}

pub fn parse(parser: &mut Parser, text: impl AsRef<[u8]>, maybe_tree: Option<&Tree>) -> Tree {
    parser.parse(text, maybe_tree).unwrap()
}

pub fn parse_rope(parser: &mut Parser, rope: &Rope, maybe_tree: Option<&Tree>) -> Tree {
    let mut lookup = |byte, _position| {
        let (chunk, chunk_byte, _, _) = rope.chunk_at_byte(byte);
        let offset = byte - chunk_byte;
        &chunk.as_bytes()[offset..]
    };
    parser
        .parse_with_options(&mut lookup, maybe_tree, None)
        .expect("failed to parse")
}

#[inline]
pub fn for_each_child<'a, E>(
    cursor: &mut TreeCursor<'a>,
    mut func: impl FnMut(usize, Node<'a>, Option<u16>, &mut TreeCursor<'a>) -> Result<(), E>,
) -> Result<(), E> {
    let mut i = 0;
    if cursor.goto_first_child() {
        loop {
            func(
                i,
                cursor.node(),
                cursor.field_id().map(|id| id.get()),
                cursor,
            )?;
            if !cursor.goto_next_sibling() {
                cursor.goto_parent();
                break;
            }
            i += 1;
        }
    };
    Ok(())
}

pub fn find_next_error(node: Node) -> Option<Node> {
    let mut cursor = node.walk();

    loop {
        let current = cursor.node();
        if current.is_error() || current.is_missing() {
            return Some(current);
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return None;
            }
        }
    }
}

pub fn format(node: Node) -> String {
    fn traverse(cursor: &mut TreeCursor, output: &mut String) {
        let indent = "    ".repeat(cursor.depth() as usize);
        let node = cursor.node();

        let start = node.start_position();
        let end = node.end_position();

        output.push_str(
            &if node.kind() == "comment" {
                console::style(node.kind())
            } else {
                console::style(node.kind()).bold()
            }
            .to_string(),
        );
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

pub mod kind {
    // SPECIAL (NAMED)
    pub const IDENTIFIER: u16 = 1; // "identifier"
    pub const COMMENT: u16 = 65; // "comment"
    pub const COMMA: u16 = 66; // "comma"
    // LITERALS (NAMED)
    pub const TRUE: u16 = 55; // "true"
    pub const FALSE: u16 = 56; // "false"
    pub const NULL: u16 = 57; // "null"
    pub const INF: u16 = 58; // "inf"
    pub const NAN: u16 = 59; // "nan"
    pub const INTEGER: u16 = 108; // "integer"
    pub const COMPLEX: u16 = 109; // "complex"
    pub const FLOAT: u16 = 110; // "float"
    pub const STRING: u16 = 112; // "string"
    pub const NA: u16 = 118; // "na"
    pub const STRING_CONTENT: u16 = 134; // "string_content"
    pub const ESCAPE_SEQUENCE: u16 = 49; // "escape_sequence"
    // LITERALS (UNAMED)
    pub const NA_LITERAL: u16 = 60; // "NA"
    pub const NA_INTEGER: u16 = 61; // "NA_integer_"
    pub const NA_REAL: u16 = 62; // "NA_real_"
    pub const NA_COMPLEX: u16 = 63; // "NA_complex_"
    pub const NA_CHARACTER: u16 = 64; // "NA_character_"
    // KEYWORDS (NAMED)
    pub const DOTS: u16 = 50; // "dots"
    pub const DOT_DOT_I: u16 = 51; // "dot_dot_i"
    pub const RETURN: u16 = 52; // "return"
    pub const NEXT: u16 = 53; // "next"
    pub const BREAK: u16 = 54; // "break"
    // KEYWORDS (UNAMED)
    pub const BACKSLASH: u16 = 2; // "\\"
    pub const FUNCTION: u16 = 3; // "function"
    pub const EQUAL: u16 = 4; // "="
    pub const IF: u16 = 5; // "if"
    pub const FOR: u16 = 6; // "for"
    pub const IN: u16 = 7; // "in"
    pub const WHILE: u16 = 8; // "while"
    pub const REPEAT: u16 = 9; // "repeat"
    pub const ELSE: u16 = 71; // "else"
    // COMPOUND EXPRESSIONS (NAMED)
    pub const PROGRAM: u16 = 81; // "program"
    pub const FUNCTION_DEFINITION: u16 = 82; // "function_definition"
    pub const PARAMETERS: u16 = 83; // "parameters"
    pub const PARAMETER: u16 = 84; // "parameter"
    pub const IF_STATEMENT: u16 = 88; // "if_statement"
    pub const FOR_STATEMENT: u16 = 89; // "for_statement"
    pub const WHILE_STATEMENT: u16 = 90; // "while_statement"
    pub const REPEAT_STATEMENT: u16 = 91; // "repeat_statement"
    pub const BRACED_EXPRESSION: u16 = 92; // "braced_expression"
    pub const PARENTHESIZED_EXPRESSION: u16 = 93; // "parenthesized_expression"
    pub const CALL: u16 = 94; // "call"
    pub const SUBSET: u16 = 95; // "subset"
    pub const SUBSET2: u16 = 96; // "subset2"
    pub const ARGUMENTS: u16 = 97; // "arguments"
    pub const ARGUMENT: u16 = 100; // "argument"
    pub const UNARY_OPERATOR: u16 = 104; // "unary_operator"
    pub const BINARY_OPERATOR: u16 = 105; // "binary_operator"
    pub const EXTRACT_OPERATOR: u16 = 106; // "extract_operator"
    pub const NAMESPACE_OPERATOR: u16 = 107; // "namespace_operator"
    // PUNCTUATION (UNAMED)
    pub const SINGLE_QUOTE: u16 = 45; // "'"
    pub const DOUBLE_QUOTE: u16 = 46; // "\""
    pub const LPAREN: u16 = 72; // "("
    pub const RPAREN: u16 = 73; // ")"
    pub const LBRACE: u16 = 74; // "{"
    pub const RBRACE: u16 = 75; // "}"
    pub const LBRACKET: u16 = 76; // "["
    pub const RBRACKET: u16 = 77; // "]"
    pub const DOUBLE_LBRACKET: u16 = 78; // "[["
    pub const DOUBLE_RBRACKET: u16 = 79; // "]]"
    // OPERATORS (UNAMED)
    pub const QUESTION: u16 = 10; // "?"
    pub const TILDE: u16 = 11; // "~"
    pub const EXCLAMATION: u16 = 12; // "!"
    pub const PLUS: u16 = 13; // "+"
    pub const MINUS: u16 = 14; // "-"
    pub const LEFT_ASSIGN: u16 = 15; // "<-"
    pub const LEFT_ASSIGN2: u16 = 16; // "<<-"
    pub const COLON_EQUAL: u16 = 17; // ":="
    pub const RIGHT_ASSIGN: u16 = 18; // "->"
    pub const RIGHT_ASSIGN2: u16 = 19; // "->>"
    pub const PIPE: u16 = 20; // "|"
    pub const AMPERSAND: u16 = 21; // "&"
    pub const DOUBLE_PIPE: u16 = 22; // "||"
    pub const DOUBLE_AMPERSAND: u16 = 23; // "&&"
    pub const LT: u16 = 24; // "<"
    pub const LTE: u16 = 25; // "<="
    pub const GT: u16 = 26; // ">"
    pub const GTE: u16 = 27; // ">="
    pub const EQEQ: u16 = 28; // "=="
    pub const NEQ: u16 = 29; // "!="
    pub const STAR: u16 = 30; // "*"
    pub const SLASH: u16 = 31; // "/"
    pub const DOUBLE_STAR: u16 = 32; // "**"
    pub const CARET: u16 = 33; // "^"
    pub const SPECIAL: u16 = 34; // "special"
    pub const PIPEBIND: u16 = 35; // "|>"
    pub const COLON: u16 = 36; // ":"
    pub const DOLLAR: u16 = 37; // "$"
    pub const AT: u16 = 38; // "@"
    pub const DOUBLE_COLON: u16 = 39; // "::"
    pub const TRIPLE_COLON: u16 = 40; // ":::"
    pub const L: u16 = 41; // "L"
    pub const I: u16 = 42; // "i"
}

pub mod field {
    pub const ALTERNATIVE: u16 = 1; // "alternative"
    pub const ARGUMENT: u16 = 2; // "argument"
    pub const ARGUMENTS: u16 = 3; // "arguments"
    pub const BODY: u16 = 4; // "body"
    pub const CLOSE: u16 = 5; // "close"
    pub const CONDITION: u16 = 6; // "condition"
    pub const CONSEQUENCE: u16 = 7; // "consequence"
    pub const CONTENT: u16 = 8; // "content"
    pub const DEFAULT: u16 = 9; // "default"
    pub const FUNCTION: u16 = 10; // "function"
    pub const LHS: u16 = 11; // "lhs"
    pub const NAME: u16 = 12; // "name"
    pub const OPEN: u16 = 13; // "open"
    pub const OPERATOR: u16 = 14; // "operator"
    pub const PARAMETER: u16 = 15; // "parameter"
    pub const PARAMETERS: u16 = 16; // "parameters"
    pub const RHS: u16 = 17; // "rhs"
    pub const SEQUENCE: u16 = 18; // "sequence"
    pub const VALUE: u16 = 19; // "value"
    pub const VARIABLE: u16 = 20; // "variable"
}

#[cfg(test)]
mod tests {
    use {super::*, tree_sitter::Language};

    #[test]
    fn check_node_ids() {
        let language: Language = tree_sitter_r::LANGUAGE.into();
        for (node_id, node_kind) in [
            // SPECIAL (NAMED)
            (kind::IDENTIFIER, "identifier"),
            (kind::COMMENT, "comment"),
            (kind::COMMA, "comma"),
            // LITERALS (NAMED)
            (kind::TRUE, "true"),
            (kind::FALSE, "false"),
            (kind::NULL, "null"),
            (kind::INF, "inf"),
            (kind::NAN, "nan"),
            (kind::INTEGER, "integer"),
            (kind::COMPLEX, "complex"),
            (kind::FLOAT, "float"),
            (kind::STRING, "string"),
            (kind::NA, "na"),
            (kind::STRING_CONTENT, "string_content"),
            (kind::ESCAPE_SEQUENCE, "escape_sequence"),
            // LITERALS (UNAMED)
            (kind::NA_LITERAL, "NA"),
            (kind::NA_INTEGER, "NA_integer_"),
            (kind::NA_REAL, "NA_real_"),
            (kind::NA_COMPLEX, "NA_complex_"),
            (kind::NA_CHARACTER, "NA_character_"),
            // KEYWORDS (NAMED)
            (kind::DOTS, "dots"),
            (kind::DOT_DOT_I, "dot_dot_i"),
            (kind::RETURN, "return"),
            (kind::NEXT, "next"),
            (kind::BREAK, "break"),
            // KEYWORDS (UNAMED)
            (kind::BACKSLASH, "\\"),
            (kind::FUNCTION, "function"),
            (kind::EQUAL, "="),
            (kind::IF, "if"),
            (kind::FOR, "for"),
            (kind::IN, "in"),
            (kind::WHILE, "while"),
            (kind::REPEAT, "repeat"),
            (kind::ELSE, "else"),
            // COMPOUND EXPRESSIONS (NAMED)
            (kind::PROGRAM, "program"),
            (kind::FUNCTION_DEFINITION, "function_definition"),
            (kind::PARAMETERS, "parameters"),
            (kind::PARAMETER, "parameter"),
            (kind::IF_STATEMENT, "if_statement"),
            (kind::FOR_STATEMENT, "for_statement"),
            (kind::WHILE_STATEMENT, "while_statement"),
            (kind::REPEAT_STATEMENT, "repeat_statement"),
            (kind::BRACED_EXPRESSION, "braced_expression"),
            (kind::PARENTHESIZED_EXPRESSION, "parenthesized_expression"),
            (kind::CALL, "call"),
            (kind::SUBSET, "subset"),
            (kind::SUBSET2, "subset2"),
            (kind::ARGUMENTS, "arguments"),
            (kind::ARGUMENT, "argument"),
            (kind::UNARY_OPERATOR, "unary_operator"),
            (kind::BINARY_OPERATOR, "binary_operator"),
            (kind::EXTRACT_OPERATOR, "extract_operator"),
            (kind::NAMESPACE_OPERATOR, "namespace_operator"),
            // PUNCTUATION (UNAMED)
            (kind::SINGLE_QUOTE, "'"),
            (kind::DOUBLE_QUOTE, "\""),
            (kind::LPAREN, "("),
            (kind::RPAREN, ")"),
            (kind::LBRACE, "{"),
            (kind::RBRACE, "}"),
            (kind::LBRACKET, "["),
            (kind::RBRACKET, "]"),
            (kind::DOUBLE_LBRACKET, "[["),
            (kind::DOUBLE_RBRACKET, "]]"),
            // OPERATORS (UNAMED)
            (kind::QUESTION, "?"),
            (kind::TILDE, "~"),
            (kind::EXCLAMATION, "!"),
            (kind::PLUS, "+"),
            (kind::MINUS, "-"),
            (kind::LEFT_ASSIGN, "<-"),
            (kind::LEFT_ASSIGN2, "<<-"),
            (kind::COLON_EQUAL, ":="),
            (kind::RIGHT_ASSIGN, "->"),
            (kind::RIGHT_ASSIGN2, "->>"),
            (kind::PIPE, "|"),
            (kind::AMPERSAND, "&"),
            (kind::DOUBLE_PIPE, "||"),
            (kind::DOUBLE_AMPERSAND, "&&"),
            (kind::LT, "<"),
            (kind::LTE, "<="),
            (kind::GT, ">"),
            (kind::GTE, ">="),
            (kind::EQEQ, "=="),
            (kind::NEQ, "!="),
            (kind::STAR, "*"),
            (kind::SLASH, "/"),
            (kind::DOUBLE_STAR, "**"),
            (kind::CARET, "^"),
            (kind::SPECIAL, "special"),
            (kind::PIPEBIND, "|>"),
            (kind::COLON, ":"),
            (kind::DOLLAR, "$"),
            (kind::AT, "@"),
            (kind::DOUBLE_COLON, "::"),
            (kind::TRIPLE_COLON, ":::"),
            (kind::L, "L"),
            (kind::I, "i"),
        ] {
            assert_eq!(language.node_kind_for_id(node_id).unwrap(), node_kind);
        }
    }

    #[test]
    fn check_field_ids() {
        let language: Language = tree_sitter_r::LANGUAGE.into();
        for (field_id, field_name) in [
            (field::ALTERNATIVE, "alternative"),
            (field::ARGUMENT, "argument"),
            (field::ARGUMENTS, "arguments"),
            (field::BODY, "body"),
            (field::CLOSE, "close"),
            (field::CONDITION, "condition"),
            (field::CONSEQUENCE, "consequence"),
            (field::CONTENT, "content"),
            (field::DEFAULT, "default"),
            (field::FUNCTION, "function"),
            (field::LHS, "lhs"),
            (field::NAME, "name"),
            (field::OPEN, "open"),
            (field::OPERATOR, "operator"),
            (field::PARAMETER, "parameter"),
            (field::PARAMETERS, "parameters"),
            (field::RHS, "rhs"),
            (field::SEQUENCE, "sequence"),
            (field::VALUE, "value"),
            (field::VARIABLE, "variable"),
        ] {
            assert_eq!(language.field_name_for_id(field_id).unwrap(), field_name);
        }
    }
}
