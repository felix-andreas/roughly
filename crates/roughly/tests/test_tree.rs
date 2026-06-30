use {
    roughly::tree::{field, kind},
    tree_sitter::Language,
};

// Pins the tree-sitter grammar's node-kind and field ids against the constants the editor layer indexes
// nodes by (`tree::kind` / `tree::field`): each constant must still resolve to its expected grammar name
// and every grammar name must be mapped, so a grammar upgrade that renumbers or adds a node is caught
// here instead of silently mis-indexing nodes at runtime.
#[test]
fn node_ids() {
    let language: Language = tree_sitter_r::LANGUAGE.into();

    for (node_id, node_kind) in KIND_MAPPING {
        assert_eq!(language.node_kind_for_id(node_id).unwrap(), node_kind);
    }

    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct NodeType {
        r#type: String,
        named: bool,
    }

    let node_types = serde_json::from_str::<Vec<NodeType>>(tree_sitter_r::NODE_TYPES).unwrap();
    let named: Vec<&str> = node_types
        .iter()
        .filter(|item| item.named)
        .map(|item| item.r#type.as_str())
        .collect();
    let unnamed: Vec<&str> = node_types
        .iter()
        .filter(|item| !item.named)
        .map(|item| item.r#type.as_str())
        .collect();

    for kind in named {
        assert!(
            KIND_MAPPING
                .into_iter()
                .any(|(_, candidate)| candidate == kind)
        )
    }

    for kind in unnamed {
        assert!(
            KIND_MAPPING
                .into_iter()
                .any(|(_, candidate)| candidate == kind)
        )
    }
}

#[test]
fn field_ids() {
    let language: Language = tree_sitter_r::LANGUAGE.into();

    for (field_id, field_name) in FIELD_MAPPING {
        assert_eq!(language.field_name_for_id(field_id).unwrap(), field_name);
    }
}

const KIND_MAPPING: [(u16, &str); 96] = [
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
    // LITERALS (UNNAMED)
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
];

const FIELD_MAPPING: [(u16, &str); 20] = [
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
];
