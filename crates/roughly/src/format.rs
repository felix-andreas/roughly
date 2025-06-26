use {
    crate::{tree, utils},
    itertools::Itertools,
    ropey::Rope,
    std::time::Instant,
    thiserror::Error,
    tree_sitter::{Node, TreeCursor},
};

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
        |maybe_node: Option<Node>| maybe_node.is_some_and(|next| next.kind() == "comment");

    // HACK: tree-sitter-r has wrong ending_position for extract with newlines before ths rhs:
    // it only includes the newline but not the rhs. this hack uses at least the correct end_position
    // see: https://github.com/users/felix-andreas/projects/5?pane=issue&itemId=100962575
    let end_position = |node: Node| {
        if node.kind() != "extract_operator" {
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
        |node: Node| node.kind() == "comment" && get_raw(node).contains("fmt: skip");

    let node = cursor.node();
    let kind = node.kind();
    let push_all_comments = |out: &mut String, cursor: &mut TreeCursor| {
        let is_first = true;
        tree::for_each_child(cursor, |_, child, _, cursor| {
            if child.kind() == "comment" {
                if is_first && node.prev_sibling().is_some() {
                    newline(out);
                }
                fmt(out, cursor)?;
                newline(out);
            }

            Ok(())
        })
    };

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

    match kind {
        "comment" => {
            let raw = get_raw(node);
            let raw = raw.trim_end();
            let mut chars = raw.chars();

            let _ = chars.next(); // Skip the '#'
            // reformat comments like #foo to # foo but keep #' foo
            match chars.next() {
                Some('\'') => match chars.next() {
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
        "argument" | "parameter" => {
            handles_comments = true;

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                            } else {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        "=" => {
                            out.push_str(" =");
                            Ok(())
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "name" => fmt(out, cursor),
                        "value" | "default" => {
                            if is_comment(maybe_prev) {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                if maybe_prev.is_some_and(|prev| prev.kind() == "=") {
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
        "arguments" | "parameters" => {
            handles_comments = true;

            let is_multiline = !same_line(node, node);
            let is_empty = node.child_count() == 2;

            let hug = if kind == "arguments" {
                field(node, "close")?.prev_sibling().is_none_or(|child| {
                    child.kind() != "comment"
                        && child.child_by_field_name("value").is_some_and(|value| {
                            value.start_position().row == node.start_position().row
                        })
                })
            } else {
                false
            };

            tree::for_each_child(cursor, |i, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if maybe_prev.is_some_and(|prev| same_line(prev, child)) {
                                space(out);
                                fmt(out, cursor)
                            } else {
                                newlines(out, child, maybe_prev);
                                fmt_indent(out, cursor)
                            }
                        }
                        "comma" => {
                            if is_multiline
                                && !hug
                                && (maybe_prev
                                    .is_none_or(|prev| ["comment", "comma"].contains(&prev.kind()))
                                    || i == 1)
                            {
                                newline(out);
                                indent(out);
                            } else if maybe_prev.is_some_and(|prev| {
                                ["argument", "parameter"].contains(&prev.kind())
                                    && prev
                                        .child(prev.child_count() - 1)
                                        .is_some_and(|last| last.kind() == "=")
                            }) {
                                space(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => fmt(out, cursor),
                        "argument" | "parameter" => {
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
                        "close" => {
                            if is_multiline && !hug && !is_empty {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        "binary_operator" => {
            handles_comments = true;

            let operator = field(node, "operator")?;
            let has_spacing = !(operator.kind() == ":" || operator.kind() == "^");
            let break_after_operator = !same_line(operator, field(node, "rhs")?);

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
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
                        "lhs" => fmt(out, cursor),
                        "operator" => {
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
                        "rhs" => {
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
        "braced_expression" => {
            handles_comments = true;

            let hug = field(node, "close")?.prev_sibling().is_none_or(|child| {
                child.kind() != "comment"
                    && child.start_position().row == node.start_position().row
                    && child.end_position().row == node.start_position().row
            });
            let is_multiline = !hug || make_multiline;
            let is_empty = node.child_count() == 2;

            tree::for_each_child(cursor, |i, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
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
                        "open" => fmt(out, cursor),
                        "body" => {
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
                        "close" => {
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
        "call" | "subset" | "subset2" => {
            handles_comments = true;

            // note: `extract_operator` has higher precedence than calls, so we must add special indentation
            // `namespace_operator` doesn't allow newlines, so there shouldn't be a problem
            let function = field(node, "function")?;
            let additional_indent = match function.kind() {
                "extract_operator" => {
                    let lhs = field(function, "lhs")?;
                    let maybe_rhs = field_optional(function, "rhs");
                    maybe_rhs.is_some_and(|rhs| !same_line(lhs, rhs))
                }
                _ => false,
            };

            tree::for_each_child(cursor, |_, child, field_name, cursor| match field_name {
                None => match child.kind() {
                    "comment" => {
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
                    "function" => fmt(out, cursor),
                    "arguments" => {
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
        "complex" => fmt_raw(node, out)?,
        "extract_operator" | "namespace_operator" => {
            handles_comments = true;

            let lhs = field(node, "lhs")?;
            let is_multiline = field_optional(node, "rhs").is_some_and(|rhs| !same_line(lhs, rhs));

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
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
                        "lhs" => fmt(out, cursor),
                        "operator" => fmt(out, cursor),
                        "rhs" => {
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
        "float" => fmt_raw(node, out)?,
        "for_statement" => {
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
                    None => match child.kind() {
                        "for" => fmt(out, cursor),
                        "in" => {
                            if prev_is_comment || loop_header_is_multiline {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                space(out);
                                fmt(out, cursor)
                            }
                        }
                        "comment" => {
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
                        "open" => {
                            indent_comments = true;
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out)
                            }
                            fmt(out, cursor)
                        }
                        "variable" => {
                            if prev_is_comment || condition_is_multiline {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        "sequence" => {
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
                        "close" => {
                            indent_comments = false;
                            if prev_is_comment || condition_is_multiline {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        "body" => {
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out)
                            }
                            if child.kind() == "braced_expression" {
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
        "function_definition" => {
            handles_comments = true;

            let is_multiline = !same_line(node, node);
            let is_same_line = same_line(field(node, "name")?, field(node, "body")?);

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "comment" => {
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
                        "name" => fmt(out, cursor),
                        "parameters" => {
                            if prev_is_comment {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        "body" => {
                            if prev_is_comment {
                                newline(out)
                            } else {
                                space(out)
                            }
                            if is_multiline {
                                if child.kind() == "braced_expression" || is_same_line {
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
        "if_statement" => {
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
                    None => match child.kind() {
                        "if" => fmt(out, cursor),
                        "else" => {
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out);
                            }
                            fmt(out, cursor)
                        }
                        "comment" => {
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
                        "open" => {
                            indent_comments = true;
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out);
                            }
                            fmt(out, cursor)
                        }
                        "condition" => {
                            if !hug {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            if !hug {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        "consequence" | "alternative" => {
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out);
                            }
                            if is_multiline {
                                if child.kind() == "braced_expression"
                                    || (child.kind() == "if_statement"
                                        && field_name == "alternative")
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
        "integer" => fmt_raw(node, out)?,
        "na" => fmt_raw(node, out)?,
        "parenthesized_expression" => {
            handles_comments = true;

            let hug = field(node, "close")?.prev_sibling().is_none_or(|child| {
                child.kind() != "comment" && child.start_position().row == node.start_position().row
            });

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
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
                        "open" => fmt(out, cursor),
                        "body" => {
                            if !hug {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        "close" => {
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
        "program" => {
            handles_comments = true;

            tree::for_each_child(cursor, |_, child, _, cursor| {
                let maybe_prev = child.prev_sibling();

                match child.kind() {
                    "comment" if maybe_prev.is_some_and(|prev| same_line(prev, child)) => {
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
        "repeat_statement" => {
            handles_comments = true;

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "repeat" => fmt(out, cursor),
                        "comment" => {
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
                            "body" => {
                                if child.kind() == "braced_expression" {
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
        "string" => {
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
        "unary_operator" => {
            handles_comments = true;

            let rhs = field(node, "rhs")?;
            let operator = field(node, "operator")?;

            let has_space = operator.kind() == "~" && rhs.kind() != "identifer";

            tree::for_each_child(&mut node.walk(), |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        // note: this branch should rarely be encountered
                        // maintaining the order of node make formatter idempotence easier
                        "comment" => {
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
                        "operator" => fmt(out, cursor),
                        "rhs" => {
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
        "while_statement" => {
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
                    None => match child.kind() {
                        "while" => fmt(out, cursor),
                        "comment" => {
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
                        "open" => {
                            indent_comments = true;
                            if prev_is_comment {
                                newline(out)
                            } else {
                                space(out)
                            };
                            fmt(out, cursor)
                        }
                        "condition" => {
                            if !hug {
                                newline(out);
                                fmt_indent(out, cursor)
                            } else {
                                fmt(out, cursor)
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            if !hug {
                                newline(out);
                            }
                            fmt(out, cursor)
                        }
                        "body" => {
                            if prev_is_comment {
                                newline(out)
                            } else {
                                space(out)
                            }
                            if child.kind() == "braced_expression" {
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
        // SIMPLE
        "break" => out.push_str("break"),
        "comma" => out.push(','),
        "dot_dot_i" => fmt_raw(node, out)?,
        "dots" => out.push_str("..."),
        "escape_sequence" => fmt_raw(node, out)?,
        "false" => out.push_str("FALSE"),
        "identifier" => fmt_raw(node, out)?,
        "inf" => out.push_str("Inf"),
        "nan" => out.push_str("NaN"),
        "next" => out.push_str("next"),
        "null" => out.push_str("NULL"),
        "return" => out.push_str("return"),
        "true" => out.push_str("TRUE"),
        unknown => {
            tracing::error!(
                "unknown node kind: {unknown}, is extra {:?}",
                node.is_extra()
            );

            return Err(FormatError::Unknown {
                kind,
                raw: get_raw(node),
            });
        }
    };

    if !handles_comments {
        let before = out.len();
        push_all_comments(out, cursor)?;

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
