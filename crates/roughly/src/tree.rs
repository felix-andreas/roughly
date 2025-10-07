use {
    ropey::{Rope, iter::Chunks},
    tree_sitter::{Node, Parser, Point, Query, QueryCursor, QueryMatches, Range, Tree, TreeCursor},
};

//
// PARSING
//

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

//
// QUERYING
//

pub fn new_query_cursor() -> QueryCursor {
    QueryCursor::new()
}

pub fn matches_rope<'cursor: 'query, 'query, 'rope, 'tree>(
    cursor: &'cursor mut QueryCursor,
    query: &'query Query,
    rope: &'rope Rope,
    node: Node<'tree>,
) -> QueryMatches<'query, 'tree, TextProvider<'rope>, &'rope [u8]> {
    cursor.matches(query, node, TextProvider(rope))
}

// based on https://github.com/zedless-editor/zed/blob/5925846cd/crates/language/src/syntax_map.rs#L219
pub struct TextProvider<'a>(pub &'a Rope);

pub struct ByteChunks<'a>(Chunks<'a>);

impl<'a> tree_sitter::TextProvider<&'a [u8]> for TextProvider<'a> {
    type I = ByteChunks<'a>;

    fn text(&mut self, node: Node) -> Self::I {
        ByteChunks(self.0.byte_slice(node.byte_range()).chunks())
    }
}

impl<'a> Iterator for ByteChunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(str::as_bytes)
    }
}

//
// UTILS
//

pub fn point_in_range(point: Point, range: Range) -> bool {
    let start = range.start_point;
    let end = range.end_point;
    start <= point && point <= end
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

pub fn find_containing_function(node: Node) -> Option<Node> {
    std::iter::successors(Some(node), |node| node.parent())
        .find(|node| node.kind_id() == kind::FUNCTION_DEFINITION)
}

pub fn try_get_identifier<'tree>(
    tree: &'tree Tree,
    line: usize,
    col: usize,
) -> Option<Node<'tree>> {
    let start = node_at_position(tree, Point::new(line, col))?;
    (start.kind_id() == kind::IDENTIFIER).then_some(start)
}

pub fn try_get_identifier_or_string<'tree>(
    tree: &'tree Tree,
    line: usize,
    col: usize,
) -> Option<(Node<'tree>, std::ops::Range<usize>)> {
    let start = node_at_position(tree, Point::new(line, col))?;
    Some(match start.kind_id() {
        kind::IDENTIFIER => (start, start.byte_range()),
        kind::STRING_CONTENT | kind::SINGLE_QUOTE | kind::DOUBLE_QUOTE => {
            let parent = start.parent()?;
            (
                parent,
                parent.child_by_field_id(field::CONTENT)?.byte_range(),
            )
        }
        _ => return None,
    })
}

pub fn node_at_position<'tree>(tree: &'tree Tree, point: Point) -> Option<Node<'tree>> {
    let node = tree.root_node().descendant_for_point_range(point, point)?;
    // If the cursor is at the very start of the program, `descendant_for_point_range` may
    // return the program node itself. Since only the program node can start at the same
    // position as e.g. an identifier, it's safe to check the first child node in this case
    match node.kind_id() {
        kind::PROGRAM => match node.child(0) {
            Some(child) if point_in_range(point, child.range()) => {
                child.descendant_for_point_range(point, point)
            }
            _ => None,
        },
        _ => Some(node),
    }
}

pub fn is_rhs_of_extract_or_namespace(node: Node) -> bool {
    node.parent().is_some_and(|parent| {
        [kind::EXTRACT_OPERATOR, kind::NAMESPACE_OPERATOR].contains(&parent.kind_id())
            && parent
                .child_by_field_id(field::RHS)
                .is_some_and(|rhs| rhs.id() == node.id())
    })
}

//
// DISPLAY AST
//

pub fn display_ast(node: Node) -> String {
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

//
// MAPPING
//

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
