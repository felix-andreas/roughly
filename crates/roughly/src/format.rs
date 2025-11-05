use {
    crate::{
        tree::{self, field, kind},
        utils,
    },
    itertools::Itertools,
    ropey::Rope,
    serde::Deserialize,
    std::{num::NonZero, time::Instant},
    thiserror::Error,
    tree_sitter::{Node, TreeCursor},
};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    pub indent_width: usize,
    pub line_ending: LineEnding,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            indent_width: 2,
            line_ending: LineEnding::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LineEnding {
    Auto,
    Lf,
    CrLf,
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
    #[error("Encountered unknown node type '{kind}' with content: \"{raw}\"")]
    UnknownKind { kind: &'static str, raw: String },
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
        LineEnding::CrLf => "\r\n",
    };

    let base_indent = " ".repeat(config.indent_width);
    let mut buffer = String::with_capacity(rope.len_bytes() * 3 / 2);
    let context = Context::new(rope, &base_indent, line_ending);
    traverse(
        &mut buffer,
        &mut node.walk(),
        &context,
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

#[derive(Debug)]
struct Context<'a> {
    rope: &'a Rope,
    indent: &'a str,
    line_ending: &'static str,
    // Pre-computed indentation strings for common nesting levels (0-10)
    indent_cache: Vec<String>,
}

impl<'a> Context<'a> {
    fn new(rope: &'a Rope, indent: &'a str, line_ending: &'static str) -> Self {
        // Pre-compute indentation strings for levels 0 through 10
        let mut indent_cache = vec![String::new()];
        for i in 1..=10 {
            indent_cache.push(indent.repeat(i));
        }
        
        Context {
            rope,
            indent,
            line_ending,
            indent_cache,
        }
    }
    
    #[inline]
    fn get_indent(&self, level: usize) -> String {
        if level < self.indent_cache.len() {
            self.indent_cache[level].clone()
        } else {
            self.indent.repeat(level)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Directive {
    Skip,
    SkipFile,
    On,
    Off,
}

fn traverse(
    out: &mut String,
    cursor: &mut TreeCursor,
    context: &Context,
    level: usize,
    make_multiline: bool,
) -> Result<(), FormatError> {
    let space = |out: &mut String| out.push(' ');
    let indent = |out: &mut String| out.push_str(context.indent);
    let newline = |out: &mut String| {
        out.push_str(context.line_ending);
        out.push_str(&context.get_indent(level));
    };
    let newlines = |out: &mut String, node: Node, maybe_prev: Option<Node>| {
        out.push_str(&context.line_ending.repeat(maybe_prev.map_or(0, |prev| {
            usize::clamp(node.start_position().row - prev.end_position().row, 1, 2)
        })));
        out.push_str(&context.get_indent(level));
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
    let fmt_raw = |out: &mut String, node: Node| {
        // note: for CRLF documents, byte_range of comment node includes \r
        // see here: https://github.com/r-lib/tree-sitter-r/pull/184
        out.push_str(get_raw(node, context.rope).trim_end_matches('\r'));
        Ok(())
    };

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

    // handle skip directives
    {
        let is_fmt_skip_comment = |node: Node| {
            node.kind_id() == kind::COMMENT
                && parse_directive(&get_raw(node, context.rope))
                    .is_some_and(|directive| directive == Directive::Skip)
        };
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
            return fmt_raw(out, node);
        }
    }

    if !node.is_named() {
        return fmt_raw(out, node);
    }

    match kind_id {
        // SPECIAL
        kind::IDENTIFIER => fmt_raw(out, node)?,
        kind::COMMENT => {
            let raw = get_raw(node, context.rope);
            let raw = raw.trim_end();

            let mut chars = raw.chars();
            let _ = chars.next(); // Skip the '#'
            // reformat comments like #foo to # foo but keep #' foo
            match chars.next() {
                Some(char @ ('\'' | '*' | ':')) => {
                    match chars.next() {
                        Some(' ') | None => out.push_str(raw),
                        // avoid formatting #'string'
                        Some(_) if char == '\'' && chars.clone().contains(&'\'') => {
                            out.push_str(raw);
                        }
                        Some(other) => {
                            out.push('#');
                            out.push(char);
                            out.push(' ');
                            out.push(other);
                            out.extend(chars);
                        }
                    }
                }
                // ! is for shebang (e.g. !#/usr/bin/env Rscript)
                Some('#' | '!' | ' ') | None => out.push_str(raw),
                Some(other) => {
                    out.push_str("# ");
                    out.push(other);
                    out.extend(chars);
                }
            }
        }
        kind::COMMA => out.push(','),
        // LITERALS
        kind::TRUE => out.push_str("TRUE"),
        kind::FALSE => out.push_str("FALSE"),
        kind::NULL => out.push_str("NULL"),
        kind::INF => out.push_str("Inf"),
        kind::NAN => out.push_str("NaN"),
        kind::INTEGER => fmt_raw(out, node)?,
        kind::COMPLEX => fmt_raw(out, node)?,
        kind::FLOAT => fmt_raw(out, node)?,
        kind::STRING => {
            if let Some(content) = field_optional(node, field::CONTENT) {
                let raw = get_raw(content, context.rope);
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
        kind::NA => fmt_raw(out, node)?,
        // both handled by STRING
        kind::ESCAPE_SEQUENCE | kind::STRING_CONTENT => unreachable!(),
        // KEYWORDS
        kind::DOTS => out.push_str("..."),
        kind::DOT_DOT_I => fmt_raw(out, node)?,
        kind::RETURN => out.push_str("return"),
        kind::NEXT => out.push_str("next"),
        kind::BREAK => out.push_str("break"),
        // COMPOUND EXPRESSIONS
        kind::ARGUMENT | kind::PARAMETER => {
            tree::for_each_child(cursor, |_, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_id {
                    None => match child.kind_id() {
                        kind::COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        kind::EQUAL => {
                            out.push_str(" =");
                            Ok(())
                        }
                        _ => unreachable!(),
                    },
                    Some(field_id) => match field_id {
                        field::NAME => fmt(out, cursor),
                        field::VALUE | field::DEFAULT => {
                            if is_comment(maybe_prev) {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                if maybe_prev.is_some_and(|prev| prev.kind_id() == kind::EQUAL) {
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
        kind::ARGUMENTS | kind::PARAMETERS => {
            let is_multiline = !same_line(node, node);
            let is_empty = node.child_count() == 2;
            let trailing_space = field(node, field::CLOSE)?
                .prev_sibling()
                .is_none_or(|child| child.kind_id() == kind::COMMA);

            let hug = field(node, field::CLOSE)?
                .prev_sibling()
                .is_none_or(|child| {
                    child.kind_id() != kind::COMMENT
                        && node
                            .children_by_field_id(
                                NonZero::new(field::ARGUMENT).unwrap(),
                                &mut node.walk(),
                            )
                            .any(|argument| {
                                argument.start_position().row == node.start_position().row
                                    && argument.end_position().row == child.end_position().row
                            })
                });

            tree::for_each_child(cursor, |i, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_id {
                    None => match child.kind_id() {
                        kind::COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                                fmt(out, cursor)
                            } else {
                                newlines(out, child, maybe_prev);
                                fmt_indent(out, cursor)
                            }
                        }
                        kind::COMMA => {
                            if is_multiline
                                && !hug
                                && (maybe_prev.is_none_or(|prev| {
                                    [kind::COMMENT, kind::COMMA].contains(&prev.kind_id())
                                }) || i == 1)
                            {
                                newline(out);
                                indent(out);
                            } else if maybe_prev.is_some_and(|prev| {
                                [kind::ARGUMENT, kind::PARAMETER].contains(&prev.kind_id())
                                    && prev
                                        .child(prev.child_count() - 1)
                                        .is_some_and(|last| last.kind_id() == kind::EQUAL)
                            }) {
                                space(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_id) => match field_id {
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
        kind::BINARY_OPERATOR => {
            let operator = field(node, field::OPERATOR)?;
            let has_spacing = ![kind::COLON, kind::CARET].contains(&operator.kind_id());
            let break_after_operator = !same_line(operator, field(node, field::RHS)?);

            tree::for_each_child(cursor, |_, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_id {
                    None => match child.kind_id() {
                        kind::COMMENT => {
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
                    Some(field_id) => match field_id {
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
        kind::BRACED_EXPRESSION => {
            let hug = field(node, field::CLOSE)?
                .prev_sibling()
                .is_none_or(|child| {
                    child.kind_id() != kind::COMMENT
                        && child.start_position().row == node.start_position().row
                        && child.end_position().row == node.start_position().row
                });
            let is_multiline = !hug || make_multiline;
            let is_empty = node.child_count() == 2;

            tree::for_each_child(cursor, |i, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_id {
                    None => match child.kind_id() {
                        kind::COMMENT => {
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
                    Some(field_id) => match field_id {
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
        kind::CALL | kind::SUBSET | kind::SUBSET2 => {
            // note: `extract_operator` has higher precedence than calls, so we must add special indentation
            // `namespace_operator` doesn't allow newlines, so there shouldn't be a problem
            let function = field(node, field::FUNCTION)?;
            let additional_indent = match function.kind_id() {
                kind::EXTRACT_OPERATOR => {
                    let lhs = field(function, field::LHS)?;
                    let maybe_rhs = field_optional(function, field::RHS);
                    maybe_rhs.is_some_and(|rhs| !same_line(lhs, rhs))
                }
                _ => false,
            };

            tree::for_each_child(cursor, |_, child, field_id, cursor| match field_id {
                None => match child.kind_id() {
                    kind::COMMENT => {
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
                Some(field_id) => match field_id {
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
        kind::EXTRACT_OPERATOR | kind::NAMESPACE_OPERATOR => {
            let lhs = field(node, field::LHS)?;
            let is_multiline =
                field_optional(node, field::RHS).is_some_and(|rhs| !same_line(lhs, rhs));

            tree::for_each_child(cursor, |_, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_id {
                    None => match child.kind_id() {
                        kind::COMMENT => {
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
                    Some(field_id) => match field_id {
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
        kind::FOR_STATEMENT => {
            let sequence = field(node, field::SEQUENCE)?;
            let open = field(node, field::OPEN)?;
            let variable = field(node, field::VARIABLE)?;

            let condition_is_multiline = !same_line(open, sequence)
                || is_comment(sequence.prev_sibling())
                || is_comment(sequence.next_sibling());

            let loop_header_is_multiline = !same_line(variable, sequence);

            let mut indent_comments = false;
            tree::for_each_child(cursor, |_, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_id {
                    None => match child.kind_id() {
                        kind::FOR => fmt(out, cursor),
                        kind::IN => {
                            if prev_is_comment || loop_header_is_multiline {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                space(out);
                                fmt(out, cursor)
                            }
                        }
                        kind::COMMENT => {
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
                    Some(field_id) => match field_id {
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
                            if child.kind_id() == kind::BRACED_EXPRESSION {
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
        kind::FUNCTION_DEFINITION => {
            let is_multiline = !same_line(node, node);
            let is_same_line = same_line(field(node, field::NAME)?, field(node, field::BODY)?);

            tree::for_each_child(cursor, |_, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_id {
                    None => match child.kind_id() {
                        kind::COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_id) => match field_id {
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
                                if child.kind_id() == kind::BRACED_EXPRESSION || is_same_line {
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
        kind::IF_STATEMENT => {
            let is_multiline = make_multiline || !same_line(node, node);

            let hug = {
                let condition = field(node, field::CONDITION)?;
                let no_comments =
                    !is_comment(condition.prev_sibling()) && !is_comment(condition.next_sibling());
                let is_same_line = same_line(field(node, field::OPEN)?, condition);
                is_same_line && no_comments
            };

            let mut indent_comments = false;
            tree::for_each_child(cursor, |_, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_id {
                    None => match child.kind_id() {
                        kind::IF => fmt(out, cursor),
                        kind::ELSE => {
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out);
                            }
                            fmt(out, cursor)
                        }
                        kind::COMMENT => {
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
                    Some(field_id) => match field_id {
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
                        field::CONSEQUENCE | field::ALTERNATIVE => {
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out);
                            }
                            if is_multiline {
                                if child.kind_id() == kind::BRACED_EXPRESSION
                                    || (child.kind_id() == kind::IF_STATEMENT
                                        && field_id == field::ALTERNATIVE)
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
        kind::PARENTHESIZED_EXPRESSION => {
            let hug = field(node, field::CLOSE)?
                .prev_sibling()
                .is_none_or(|child| {
                    child.kind_id() != kind::COMMENT
                        && child.start_position().row == node.start_position().row
                });

            tree::for_each_child(cursor, |_, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_id {
                    None => match child.kind_id() {
                        kind::COMMENT => {
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
                    Some(field_id) => match field_id {
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
        kind::PROGRAM => {
            let mut enabled = true;
            let mut maybe_directive = None;
            if node.child(0).is_some_and(|child| {
                child.kind_id() == kind::COMMENT
                    && parse_directive(&get_raw(child, context.rope))
                        .is_some_and(|directive| directive == Directive::SkipFile)
            }) {
                return fmt_raw(out, node);
            }

            tree::for_each_child(cursor, |_, child, _, cursor| {
                // Delay toggling the `enabled` flag until after handling newlines.
                // This ensures that any preceding newlines are attributed to the previous child
                match maybe_directive {
                    Some(Directive::On) => enabled = true,
                    Some(Directive::Off) => enabled = false,
                    _ => {}
                }

                maybe_directive = match child.kind_id() {
                    kind::COMMENT => parse_directive(&get_raw(child, context.rope)),
                    _ => None,
                };

                let maybe_prev = child.prev_sibling();
                if enabled {
                    match child.kind_id() {
                        kind::COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newlines(out, child, maybe_prev);
                            }
                        }
                        _ => {
                            newlines(out, child, maybe_prev);
                        }
                    }
                    fmt(out, cursor)
                } else {
                    if let Some(prev) = maybe_prev {
                        out.push_str(
                            &context
                                .line_ending
                                .repeat(child.start_position().row - prev.end_position().row),
                        )
                    }
                    // we also want to format current directive comment
                    if maybe_directive.is_some() {
                        fmt(out, cursor)
                    } else {
                        fmt_raw(out, child)
                    }
                }
            })?;

            newline(out);
        }
        kind::REPEAT_STATEMENT => {
            tree::for_each_child(cursor, |_, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_id {
                    None => match child.kind_id() {
                        kind::REPEAT => fmt(out, cursor),
                        kind::COMMENT => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_id) => {
                        if prev_is_comment {
                            newline(out);
                        } else {
                            space(out);
                        }
                        match field_id {
                            field::BODY => {
                                if child.kind_id() == kind::BRACED_EXPRESSION {
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
        kind::UNARY_OPERATOR => {
            let rhs = field(node, field::RHS)?;
            let operator = field(node, field::OPERATOR)?;

            let has_space = operator.kind_id() == kind::TILDE && rhs.kind_id() != kind::IDENTIFIER;

            tree::for_each_child(&mut node.walk(), |_, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_id {
                    None => match child.kind_id() {
                        // note: this branch should rarely be encountered
                        // maintaining the order of node make formatter idempotence easier
                        kind::COMMENT => {
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
                    Some(field_id) => match field_id {
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
        kind::WHILE_STATEMENT => {
            let hug = {
                let condition = field(node, field::CONDITION)?;
                let no_comments =
                    !is_comment(condition.prev_sibling()) && !is_comment(condition.next_sibling());
                let is_same_line = same_line(field(node, field::OPEN)?, condition);
                is_same_line && no_comments
            };

            let mut indent_comments = false;
            tree::for_each_child(cursor, |_, child, field_id, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_id {
                    None => match child.kind_id() {
                        kind::WHILE => fmt(out, cursor),
                        kind::COMMENT => {
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
                    Some(field_id) => match field_id {
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
                            if child.kind_id() == kind::BRACED_EXPRESSION {
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

            return Err(FormatError::UnknownKind {
                kind: node.kind(),
                raw: get_raw(node, context.rope),
            });
        }
    };

    Ok(())
}

fn get_raw(node: Node, rope: &Rope) -> String {
    rope.byte_slice(node.byte_range()).to_string()
}

fn field<'a>(node: Node<'a>, field_id: u16) -> Result<Node<'a>, FormatError> {
    node.child_by_field_id(field_id)
        .ok_or(FormatError::MissingField {
            kind: node.kind(),
            field: node
                .language()
                .field_name_for_id(field_id)
                .unwrap_or("unknown"),
        })
}

fn field_optional<'a>(node: Node<'a>, field_id: u16) -> Option<Node<'a>> {
    node.child_by_field_id(field_id)
}

fn is_comment(maybe_node: Option<Node>) -> bool {
    maybe_node.is_some_and(|node| node.kind_id() == kind::COMMENT)
}

fn same_line(a: Node, b: Node) -> bool {
    end_position(a).row == b.start_position().row
}

// HACK: tree-sitter-r has wrong ending_position for extract with newlines before ths rhs:
// it only includes the newline but not the rhs. this hack uses at least the correct end_position
// see: https://github.com/users/felix-andreas/projects/5?pane=issue&itemId=100962575
fn end_position(node: Node) -> tree_sitter::Point {
    if node.kind_id() != kind::EXTRACT_OPERATOR {
        return node.end_position();
    }

    field_optional(node, field::RHS)
        .map(|rhs| rhs.end_position())
        .or_else(|| field_optional(node, field::OPERATOR).map(|operator| operator.end_position()))
        // note: this case is unexpected
        .unwrap_or_else(|| node.end_position())
}

fn parse_directive(text: &str) -> Option<Directive> {
    text.trim_start_matches(|c: char| c.is_whitespace() || c == '#')
        .strip_prefix("fmt:")
        .and_then(|rhs| match rhs.trim() {
            "skip" => Some(Directive::Skip),
            "skip-file" => Some(Directive::SkipFile),
            "skip file" => Some(Directive::SkipFile),
            "on" => Some(Directive::On),
            "off" => Some(Directive::Off),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_directive_skip() {
        assert_eq!(parse_directive("# fmt: skip"), Some(Directive::Skip));
        assert_eq!(
            parse_directive("# fmt: skip-file"),
            Some(Directive::SkipFile)
        );
        assert_eq!(parse_directive("# fmt: on"), Some(Directive::On));
        assert_eq!(parse_directive("# fmt: off"), Some(Directive::Off));

        // check whitespace variations
        assert_eq!(parse_directive("#fmt:skip"), Some(Directive::Skip));
        assert_eq!(parse_directive("# fmt:skip "), Some(Directive::Skip));
        assert_eq!(parse_directive(" # fmt: skip "), Some(Directive::Skip));
    }

    #[test]
    fn parse_directive_none() {
        assert_eq!(parse_directive("# fmt:unknown"), None);
        assert_eq!(parse_directive("# something else"), None);
        assert_eq!(parse_directive(""), None);
        assert_eq!(parse_directive(""), None);
    }
}
