use {
    ropey::Rope,
    tree_sitter::{Parser, Tree},
};

//
// QUERYING
//

pub fn new_parser() -> Result<Parser, tree_sitter::LanguageError> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_r::LANGUAGE.into())?;
    Ok(parser)
}

pub fn parse_rope(parser: &mut Parser, rope: &Rope, previous_tree: Option<&Tree>) -> Option<Tree> {
    let mut lookup = |byte, _position| {
        let (chunk, chunk_byte, _, _) = rope.chunk_at_byte(byte);
        let offset = byte - chunk_byte;
        &chunk.as_bytes()[offset..]
    };

    parser.parse_with_options(&mut lookup, previous_tree, None)
}

//
// MAPPING
//

pub mod kind {
    // SPECIAL (NAMED)
    pub const IDENTIFIER: u16 = 1; // "identifier"
    pub const COMMENT: u16 = 64; // "comment"
    pub const COMMA: u16 = 65; // "comma"
    // LITERALS (NAMED)
    pub const TRUE: u16 = 54; // "true"
    pub const FALSE: u16 = 55; // "false"
    pub const NULL: u16 = 56; // "null"
    pub const INF: u16 = 57; // "inf"
    pub const NAN: u16 = 58; // "nan"
    pub const INTEGER: u16 = 109; // "integer"
    pub const COMPLEX: u16 = 110; // "complex"
    pub const FLOAT: u16 = 111; // "float"
    pub const STRING: u16 = 113; // "string"
    pub const NA: u16 = 120; // "na"
    pub const STRING_OPEN: u16 = 69; // "string_open"
    pub const STRING_CONTENT: u16 = 70; // "string_content"
    pub const STRING_CLOSE: u16 = 71; // "string_close"
    pub const ESCAPE_SEQUENCE: u16 = 49; // "escape_sequence"
    // LITERALS (UNNAMED)
    pub const NA_LITERAL: u16 = 59; // "NA"
    pub const NA_INTEGER: u16 = 60; // "NA_integer_"
    pub const NA_REAL: u16 = 61; // "NA_real_"
    pub const NA_COMPLEX: u16 = 62; // "NA_complex_"
    pub const NA_CHARACTER: u16 = 63; // "NA_character_"
    // KEYWORDS (NAMED)
    pub const DOTS: u16 = 50; // "dots"
    pub const DOT_DOT_I: u16 = 51; // "dot_dot_i"
    pub const NEXT: u16 = 52; // "next"
    pub const BREAK: u16 = 53; // "break"
    // KEYWORDS (UNAMED)
    pub const BACKSLASH: u16 = 2; // "\\"
    pub const FUNCTION: u16 = 3; // "function"
    pub const EQUAL: u16 = 4; // "="
    pub const IF: u16 = 5; // "if"
    pub const FOR: u16 = 6; // "for"
    pub const IN: u16 = 7; // "in"
    pub const WHILE: u16 = 8; // "while"
    pub const REPEAT: u16 = 9; // "repeat"
    pub const ELSE: u16 = 72; // "else"
    // COMPOUND EXPRESSIONS (NAMED)
    pub const PROGRAM: u16 = 82; // "program"
    pub const FUNCTION_DEFINITION: u16 = 83; // "function_definition"
    pub const PARAMETERS: u16 = 84; // "parameters"
    pub const PARAMETER: u16 = 85; // "parameter"
    pub const IF_STATEMENT: u16 = 89; // "if_statement"
    pub const FOR_STATEMENT: u16 = 90; // "for_statement"
    pub const WHILE_STATEMENT: u16 = 91; // "while_statement"
    pub const REPEAT_STATEMENT: u16 = 92; // "repeat_statement"
    pub const BRACED_EXPRESSION: u16 = 93; // "braced_expression"
    pub const PARENTHESIZED_EXPRESSION: u16 = 94; // "parenthesized_expression"
    pub const CALL: u16 = 95; // "call"
    pub const SUBSET: u16 = 96; // "subset"
    pub const SUBSET2: u16 = 97; // "subset2"
    pub const ARGUMENTS: u16 = 98; // "arguments"
    pub const ARGUMENT: u16 = 101; // "argument"
    pub const UNARY_OPERATOR: u16 = 105; // "unary_operator"
    pub const BINARY_OPERATOR: u16 = 106; // "binary_operator"
    pub const EXTRACT_OPERATOR: u16 = 107; // "extract_operator"
    pub const NAMESPACE_OPERATOR: u16 = 108; // "namespace_operator"
    // PUNCTUATION (UNNAMED)
    pub const LPAREN: u16 = 73; // "("
    pub const RPAREN: u16 = 74; // ")"
    pub const LBRACE: u16 = 75; // "{"
    pub const RBRACE: u16 = 76; // "}"
    pub const LBRACKET: u16 = 77; // "["
    pub const RBRACKET: u16 = 78; // "]"
    pub const DOUBLE_LBRACKET: u16 = 79; // "[["
    pub const DOUBLE_RBRACKET: u16 = 80; // "]]"
    // OPERATORS (UNNAMED)
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
