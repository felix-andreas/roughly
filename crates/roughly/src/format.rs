use {
    crate::{tree, utils},
    itertools::Itertools,
    ropey::Rope,
    std::time::Instant,
    thiserror::Error,
    tree_sitter::{Node, TreeCursor},
};

pub mod node_kind {
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

#[derive(Debug, Clone, Copy)]
pub struct Config<'a> {
    pub indent: &'a str,
    pub line_ending: LineEnding,
}

#[derive(Debug, Clone, Copy)]
pub enum LineEnding {
    Auto,
    Lf,
    Crlf,
}

#[derive(Error, Debug)]
pub enum FormatError {
    #[error("Syntax error: Unexpected {kind} at line {line}, column {col}")]
    SyntaxError {
        kind: &'static str,
        line: usize,
        col: usize,
    },
    #[error("Missing node: Expected {kind} at line {line}, column {col}")]
    Missing {
        kind: &'static str,
        line: usize,
        col: usize,
    },
    #[error("Missing required field '{field}' in node of type '{kind}'")]
    MissingField {
        kind: &'static str,
        field: &'static str,
    },
    #[error("Unhandled comment found at line {line}, column {col}: \"{raw}\"")]
    UnhandledComment {
        raw: String,
        line: usize,
        col: usize,
    },
    #[error("Encountered unknown node type '{kind}' with content: \"{raw}\"")]
    Unknown { kind: &'static str, raw: String },
}

pub fn format(node: Node, rope: &Rope, config: Config) -> Result<String, FormatError> {
    let start = Instant::now();

    if node.has_error() {
        let error = tree::find_next_error(node).unwrap();

        let line = error.start_position().row;
        let col = error.start_position().column;
        let kind = error.kind();

        return Err(if error.is_missing() {
            FormatError::Missing { kind, line, col }
        } else {
            FormatError::SyntaxError { kind, line, col }
        });
    }

    let line_ending = match config.line_ending {
        LineEnding::Auto => rope
            .chars()
            .tuple_windows()
            .find_map(|(a, b)| match b {
                '\n' => Some(match a {
                    '\r' => "\r\n",
                    _ => "\n",
                }),
                _ => None,
            })
            .unwrap_or("\n"),
        LineEnding::Lf => "\n",
        LineEnding::Crlf => "\r\n",
    };

    let mut buffer = String::with_capacity(rope.len_bytes() * 3 / 2);
    traverse(
        &mut buffer,
        &mut node.walk(),
        Context {
            rope,
            indent: config.indent,
            line_ending,
        },
        0,
        false,
    )?;

    let elapsed = start.elapsed();
    tracing::debug!(
        "formatted {} lines in {} ms ({}/s)",
        rope.len_lines(),
        elapsed.as_millis(),
        utils::human_bytes(rope.len_bytes() as f64 / elapsed.as_secs_f64())
    );

    Ok(buffer)
}

#[derive(Debug, Clone, Copy)]
struct Context<'a> {
    rope: &'a Rope,
    indent: &'a str,
    line_ending: &'static str,
}

fn traverse(
    out: &mut String,
    cursor: &mut TreeCursor,
    context: Context,
    level: usize,
    make_multiline: bool,
) -> Result<(), FormatError> {
    use node_kind::*;

    let space = |out: &mut String| out.push(' ');
    let indent = |out: &mut String| out.push_str(context.indent);
    let newline = |out: &mut String| {
        out.push_str(context.line_ending);
        out.push_str(&context.indent.repeat(level));
    };
    let newlines = |out: &mut String, node: Node, maybe_prev: Option<Node>| {
        out.push_str(&context.line_ending.repeat(maybe_prev.map_or(0, |prev| {
            usize::clamp(node.start_position().row - prev.end_position().row, 1, 2)
        })));
        out.push_str(&context.indent.repeat(level));
    };

    let fmt_with =
        |out: &mut String, cursor: &mut TreeCursor, level: usize, make_multiline: bool| {
            traverse(out, cursor, context, level, make_multiline)
        };

    let fmt = |out: &mut String, cursor: &mut TreeCursor| fmt_with(out, cursor, level, false);
    let fmt_indent = |out: &mut String, cursor: &mut TreeCursor| {
        indent(out);
        fmt_with(out, cursor, level + 1, false)
    };
    let fmt_indent_skip_first =
        |out: &mut String, cursor: &mut TreeCursor| fmt_with(out, cursor, level + 1, false);
    let fmt_multiline =
        |out: &mut String, cursor: &mut TreeCursor| fmt_with(out, cursor, level, true);
    let fmt_braces = |out: &mut String, cursor: &mut TreeCursor| {
        out.push('{');
        newline(out);
        fmt_indent(out, cursor)?;
        newline(out);
        out.push('}');
        Ok(())
    };

    let get_raw = |node: Node| context.rope.byte_slice(node.byte_range()).to_string();
    let fmt_raw = |node: Node, out: &mut String| {
        // note: for CRLF documents, byte_range of comment node includes \r
        out.push_str(get_raw(node).trim_end_matches('\r'));
        Ok(())
    };

    fn field<'a>(node: Node<'a>, field_name: &'static str) -> Result<Node<'a>, FormatError> {
        node.child_by_field_name(field_name)
            .ok_or(FormatError::MissingField {
                kind: node.kind(),
                field: field_name,
            })
    }

    fn field_optional<'a>(node: Node<'a>, field_name: &'static str) -> Option<Node<'a>> {
        node.child_by_field_name(field_name)
    }

    let is_comment =
        |maybe_node: Option<Node>| maybe_node.is_some_and(|next| next.kind_id() == COMMENT);

    // HACK: tree-sitter-r has wrong ending_position for extract with newlines before ths rhs:
    // it only includes the newline but not the rhs. this hack uses at least the correct end_position
    // see: https://github.com/users/felix-andreas/projects/5?pane=issue&itemId=100962575
    let end_position = |node: Node| {
        if node.kind_id() != EXTRACT_OPERATOR {
            return node.end_position();
        }

        field_optional(node, "rhs")
            .map(|rhs| rhs.end_position())
            .or_else(|| field_optional(node, "operator").map(|operator| operator.end_position()))
            // note: this case is unexpected
            .unwrap_or_else(|| node.end_position())
    };

    let same_line = |a: Node, b: Node| end_position(a).row == b.start_position().row;

    let is_fmt_skip_comment =
        |node: Node| node.kind_id() == COMMENT && get_raw(node).contains("fmt: skip");

    let node = cursor.node();
    let kind_id = node.kind_id();

    if node.is_error() {
        return Err(FormatError::SyntaxError {
            kind: node.kind(),
            line: node.start_position().row,
            col: node.start_position().column,
        });
    }

    if node.is_missing() {
        return Err(FormatError::Missing {
            kind: node.kind(),
            line: node.start_position().row,
            col: node.start_position().column,
        });
    }

    // check if prev or next node is fmt-skip directive
    {
        let prev_is_fmt_skip = node.prev_sibling().is_some_and(|prev| {
            is_fmt_skip_comment(prev)
                && prev
                    .prev_sibling()
                    .is_none_or(|before_prev| !same_line(before_prev, prev))
        });
        let next_is_fmt_skip = node
            .next_sibling()
            .is_some_and(|next| is_fmt_skip_comment(next) && same_line(node, next));

        if prev_is_fmt_skip || next_is_fmt_skip {
            return fmt_raw(node, out);
        }
    }

    if !node.is_named() {
        return fmt_raw(node, out);
    }

    let mut handles_comments = false;

    match kind_id {
        // SPECIAL
        IDENTIFIER => fmt_raw(node, out)?,
        COMMENT => {
            let raw = get_raw(node);
            let raw = raw.trim_end();
            let mut chars = raw.chars();

            let _ = chars.next(); // Skip the '#'
            // reformat comments like #foo to # foo but keep #' foo
            match chars.next() {
                Some('\'' | '*' | ':') => match chars.next() {
                    Some(' ') => out.push_str(raw),
                    Some(other) => {
                        let rest = chars.collect::<String>();
                        // avoid formatting #'foo'
                        if rest.contains('\'') {
                            out.push_str(raw);
                        } else {
                            out.push_str("#' ");
                            out.push(other);
                            out.push_str(&rest);
                        }
                    }
                    None => out.push_str("#'"),
                },
                Some('#' | '!' | ' ') => out.push_str(raw),
                Some(other) => {
                    out.push_str("# ");
                    out.push(other);
                    out.push_str(&chars.collect::<String>());
                }
                None => out.push('#'),
            }
        }
        COMMA => out.push(','),
        // LITERALS
        TRUE => out.push_str("TRUE"),
        FALSE => out.push_str("FALSE"),
        NULL => out.push_str("NULL"),
        INF => out.push_str("Inf"),
        NAN => out.push_str("NaN"),
        INTEGER => fmt_raw(node, out)?,
        COMPLEX => fmt_raw(node, out)?,
        FLOAT => fmt_raw(node, out)?,
        STRING => {
            if let Some(content) = field_optional(node, "content") {
                let raw = get_raw(content);
                let mut all_quotes_escaped = true;
                let mut prev_was_escape = false;
                for char in raw.chars() {
                    if char == '"' {
                        all_quotes_escaped &= prev_was_escape;
                    }
                    prev_was_escape = char == '\\' && !prev_was_escape;
                }
                let quote = if all_quotes_escaped { '"' } else { '\'' };
                out.push(quote);
                out.push_str(&raw);
                out.push(quote);
            } else {
                out.push_str(r#""""#);
            }
        }
        NA => fmt_raw(node, out)?,
        // both handled by STRING
        ESCAPE_SEQUENCE | STRING_CONTENT => unreachable!(),
        // KEYWORDS
        DOTS => out.push_str("..."),
        DOT_DOT_I => fmt_raw(node, out)?,
        RETURN => out.push_str("return"),
        NEXT => out.push_str("next"),
        BREAK => out.push_str("break"),
        // COMPOUND EXPRESSIONS
        ARGUMENT | PARAMETER => {
            handles_comments = true;

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind_id() {
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        EQUAL => {
                            out.push_str(" =");
                            Ok(())
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::NAME => fmt(out, cursor),
                        field::VALUE | field::DEFAULT => {
                            if is_comment(maybe_prev) {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                if maybe_prev.is_some_and(|prev| prev.kind_id() == EQUAL) {
                                    space(out);
                                }
                                fmt(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        ARGUMENTS | PARAMETERS => {
            handles_comments = true;

            let is_multiline = !same_line(node, node);
            let is_empty = node.child_count() == 2;
            let mut trailing_space = false;

            let hug = (kind_id == ARGUMENTS) && {
                field(node, "close")?.prev_sibling().is_none_or(|child| {
                    trailing_space = child.kind_id() == COMMA;
                    child.kind_id() != COMMENT
                        && child.child_by_field_name("value").is_some_and(|value| {
                            value.start_position().row == node.start_position().row
                        })
                })
            };

            tree::for_each_child(cursor, |i, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind_id() {
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                                fmt(out, cursor)
                            } else {
                                newlines(out, child, maybe_prev);
                                fmt_indent(out, cursor)
                            }
                        }
                        COMMA => {
                            if is_multiline
                                && !hug
                                && (maybe_prev
                                    .is_none_or(|prev| [COMMENT, COMMA].contains(&prev.kind_id()))
                                    || i == 1)
                            {
                                newline(out);
                                indent(out);
                            } else if maybe_prev.is_some_and(|prev| {
                                [ARGUMENT, PARAMETER].contains(&prev.kind_id())
                                    && prev
                                        .child(prev.child_count() - 1)
                                        .is_some_and(|last| last.kind_id() == EQUAL)
                            }) {
                                space(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::OPEN => fmt(out, cursor),
                        field::ARGUMENT | field::PARAMETER => {
                            if i == 1 {
                                if is_multiline && !hug {
                                    newline(out);
                                }
                            } else if is_multiline && !hug {
                                newlines(out, child, maybe_prev);
                            } else {
                                space(out);
                            }

                            if is_multiline && !hug {
                                fmt_indent(out, cursor)
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        field::CLOSE => {
                            if is_multiline {
                                if !hug && !is_empty {
                                    newline(out);
                                }
                            } else if trailing_space {
                                space(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        BINARY_OPERATOR => {
            handles_comments = true;

            let operator = field(node, "operator")?;
            let has_spacing = !(operator.kind_id() == COLON || operator.kind_id() == CARET);
            let break_after_operator = !same_line(operator, field(node, "rhs")?);

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind_id() {
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                                fmt(out, cursor)
                            } else {
                                newline(out);
                                fmt_indent(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::LHS => fmt(out, cursor),
                        field::OPERATOR => {
                            if is_comment(maybe_prev) {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                if has_spacing {
                                    space(out);
                                }
                                fmt(out, cursor)
                            }
                        }
                        field::RHS => {
                            if break_after_operator {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                if has_spacing {
                                    space(out)
                                }
                                fmt(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        BRACED_EXPRESSION => {
            handles_comments = true;

            let hug = field(node, "close")?.prev_sibling().is_none_or(|child| {
                child.kind_id() != COMMENT
                    && child.start_position().row == node.start_position().row
                    && child.end_position().row == node.start_position().row
            });
            let is_multiline = !hug || make_multiline;
            let is_empty = node.child_count() == 2;

            tree::for_each_child(cursor, |i, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind_id() {
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                                fmt(out, cursor)
                            } else {
                                newlines(out, child, maybe_prev);
                                fmt_indent(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::OPEN => fmt(out, cursor),
                        field::BODY => {
                            if is_multiline {
                                if i == 1 {
                                    newline(out)
                                } else {
                                    newlines(out, child, maybe_prev);
                                }
                                fmt_indent(out, cursor)
                            } else {
                                if i == 1 {
                                    space(out)
                                } else {
                                    out.push_str("; ");
                                }
                                fmt(out, cursor)
                            }
                        }
                        field::CLOSE => {
                            if !is_empty {
                                if is_multiline {
                                    newline(out)
                                } else {
                                    space(out)
                                };
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        CALL | SUBSET | SUBSET2 => {
            handles_comments = true;

            // note: `extract_operator` has higher precedence than calls, so we must add special indentation
            // `namespace_operator` doesn't allow newlines, so there shouldn't be a problem
            let function = field(node, "function")?;
            let additional_indent = match function.kind_id() {
                EXTRACT_OPERATOR => {
                    let lhs = field(function, "lhs")?;
                    let maybe_rhs = field_optional(function, "rhs");
                    maybe_rhs.is_some_and(|rhs| !same_line(lhs, rhs))
                }
                _ => false,
            };

            tree::for_each_child(cursor, |_, child, field_name, cursor| match field_name {
                None => match child.kind_id() {
                    COMMENT => {
                        let maybe_prev = child.prev_sibling();
                        if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                            space(out);
                            fmt(out, cursor)
                        } else {
                            newline(out);
                            fmt_indent(out, cursor)
                        }
                    }
                    _ => unreachable!(),
                },
                Some(field_name) => match field_name {
                    field::FUNCTION => fmt(out, cursor),
                    field::ARGUMENTS => {
                        if additional_indent {
                            fmt_indent_skip_first(out, cursor)
                        } else {
                            fmt(out, cursor)
                        }
                    }
                    _ => unreachable!(),
                },
            })?;
        }
        EXTRACT_OPERATOR | NAMESPACE_OPERATOR => {
            handles_comments = true;

            let lhs = field(node, "lhs")?;
            let is_multiline = field_optional(node, "rhs").is_some_and(|rhs| !same_line(lhs, rhs));

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind_id() {
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                                fmt(out, cursor)
                            } else {
                                newline(out);
                                fmt_indent(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::LHS => fmt(out, cursor),
                        field::OPERATOR => fmt(out, cursor),
                        field::RHS => {
                            if is_multiline {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        FOR_STATEMENT => {
            handles_comments = true;

            let sequence = field(node, "sequence")?;
            let open = field(node, "open")?;
            let variable = field(node, "variable")?;

            let condition_is_multiline = !same_line(open, sequence)
                || is_comment(sequence.prev_sibling())
                || is_comment(sequence.next_sibling());

            let loop_header_is_multiline = !same_line(variable, sequence);

            let mut indent_comments = false;
            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind_id() {
                        FOR => fmt(out, cursor),
                        IN => {
                            if prev_is_comment || loop_header_is_multiline {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                space(out);
                                fmt(out, cursor)
                            }
                        }
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                                if indent_comments {
                                    indent(out);
                                }
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::OPEN => {
                            indent_comments = true;
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out)
                            }
                            fmt(out, cursor)
                        }
                        field::VARIABLE => {
                            if prev_is_comment || condition_is_multiline {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        field::SEQUENCE => {
                            if prev_is_comment {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                space(out);
                                if condition_is_multiline {
                                    fmt_indent_skip_first(out, cursor)
                                } else {
                                    fmt(out, cursor)
                                }
                            }
                        }
                        field::CLOSE => {
                            indent_comments = false;
                            if prev_is_comment || condition_is_multiline {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        field::BODY => {
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out);
                            }
                            if child.kind_id() == BRACED_EXPRESSION {
                                fmt_multiline(out, cursor)
                            } else {
                                fmt_braces(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        FUNCTION_DEFINITION => {
            handles_comments = true;

            let is_multiline = !same_line(node, node);
            let is_same_line = same_line(field(node, "name")?, field(node, "body")?);

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind_id() {
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::NAME => fmt(out, cursor),
                        field::PARAMETERS => {
                            if prev_is_comment {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        field::BODY => {
                            if prev_is_comment {
                                newline(out)
                            } else {
                                space(out)
                            }
                            if is_multiline {
                                if child.kind_id() == BRACED_EXPRESSION || is_same_line {
                                    fmt_multiline(out, cursor)
                                } else {
                                    fmt_braces(out, cursor)
                                }
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        IF_STATEMENT => {
            handles_comments = true;

            let is_multiline = make_multiline || !same_line(node, node);

            let hug = {
                let condition = field(node, "condition")?;
                let no_comments =
                    !is_comment(condition.prev_sibling()) && !is_comment(condition.next_sibling());
                let is_same_line = same_line(field(node, "open")?, condition);
                is_same_line && no_comments
            };

            let mut indent_comments = false;
            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind_id() {
                        IF => fmt(out, cursor),
                        ELSE => {
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out);
                            }
                            fmt(out, cursor)
                        }
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                                if indent_comments {
                                    indent(out);
                                }
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::OPEN => {
                            indent_comments = true;
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out);
                            }
                            fmt(out, cursor)
                        }
                        field::CONDITION => {
                            if !hug {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        field::CLOSE => {
                            indent_comments = false;
                            if !hug {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        f if f == field::CONSEQUENCE || f == field::ALTERNATIVE => {
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out);
                            }
                            if is_multiline {
                                if child.kind_id() == BRACED_EXPRESSION
                                    || (child.kind_id() == IF_STATEMENT
                                        && field_name == field::ALTERNATIVE)
                                {
                                    fmt_multiline(out, cursor)
                                } else {
                                    fmt_braces(out, cursor)
                                }
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        PARENTHESIZED_EXPRESSION => {
            handles_comments = true;

            let hug = field(node, "close")?.prev_sibling().is_none_or(|child| {
                child.kind_id() != COMMENT
                    && child.start_position().row == node.start_position().row
            });

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind_id() {
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                                out.push_str(context.indent);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::OPEN => fmt(out, cursor),
                        field::BODY => {
                            if !hug {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        field::CLOSE => {
                            if !hug {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        PROGRAM => {
            handles_comments = true;

            tree::for_each_child(cursor, |_, child, _, cursor| {
                let maybe_prev = child.prev_sibling();

                match child.kind_id() {
                    COMMENT if maybe_prev.is_some_and(|prev| same_line(prev, child)) => {
                        space(out);
                    }
                    _ => {
                        newlines(out, child, maybe_prev);
                    }
                }
                fmt(out, cursor)
            })?;
            newline(out);
        }
        REPEAT_STATEMENT => {
            handles_comments = true;

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind_id() {
                        REPEAT => fmt(out, cursor),
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => {
                        if prev_is_comment {
                            newline(out);
                        } else {
                            space(out);
                        }
                        match field_name {
                            field::BODY => {
                                if child.kind_id() == BRACED_EXPRESSION {
                                    fmt_multiline(out, cursor)
                                } else {
                                    fmt_braces(out, cursor)
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            })?;
        }
        UNARY_OPERATOR => {
            handles_comments = true;

            let rhs = field(node, "rhs")?;
            let operator = field(node, "operator")?;

            let has_space = operator.kind_id() == TILDE && rhs.kind_id() != IDENTIFIER;

            tree::for_each_child(&mut node.walk(), |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind_id() {
                        // note: this branch should rarely be encountered
                        // maintaining the order of node make formatter idempotence easier
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                                fmt(out, cursor)
                            } else {
                                newlines(out, child, maybe_prev);
                                fmt_indent(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::OPERATOR => fmt(out, cursor),
                        field::RHS => {
                            if is_comment(maybe_prev) {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                if has_space {
                                    space(out);
                                }
                                fmt(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        WHILE_STATEMENT => {
            handles_comments = true;

            let hug = {
                let condition = field(node, "condition")?;
                let no_comments =
                    !is_comment(condition.prev_sibling()) && !is_comment(condition.next_sibling());
                let is_same_line = same_line(field(node, "open")?, condition);
                is_same_line && no_comments
            };

            let mut indent_comments = false;
            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind_id() {
                        WHILE => fmt(out, cursor),
                        COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                                if indent_comments {
                                    indent(out);
                                }
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        field::OPEN => {
                            indent_comments = true;
                            if prev_is_comment {
                                newline(out)
                            } else {
                                space(out)
                            };
                            fmt(out, cursor)
                        }
                        field::CONDITION => {
                            if !hug {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        field::CLOSE => {
                            indent_comments = false;
                            if !hug {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        field::BODY => {
                            if prev_is_comment {
                                newline(out)
                            } else {
                                space(out)
                            }
                            if child.kind_id() == BRACED_EXPRESSION {
                                fmt_multiline(out, cursor)
                            } else {
                                fmt_braces(out, cursor)
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        _ => {
            tracing::error!(
                "unknown node kind: {} (id: {}), is extra {:?}",
                node.kind(),
                kind_id,
                node.is_extra()
            );

            return Err(FormatError::Unknown {
                kind: node.kind(),
                raw: get_raw(node),
            });
        }
    };

    if !handles_comments {
        let before = out.len();
        {
            tree::for_each_child(cursor, |_, child, _, cursor| {
                if child.kind_id() == COMMENT {
                    if node.prev_sibling().is_some() {
                        newline(out);
                    }
                    fmt(out, cursor)?;
                    newline(out);
                }
                Ok::<(), FormatError>(())
            })?;
        }

        if out.len() != before {
            let start = node.start_position();
            return Err(FormatError::UnhandledComment {
                raw: get_raw(node),
                line: start.row,
                col: start.column,
            });
        }
    }
    Ok(())
}
