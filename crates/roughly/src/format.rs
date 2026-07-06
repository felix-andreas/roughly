use {
    crate::{
        tree::{self, field, kind},
        utils,
    },
    analysis::{
        interner::Interner,
        type_syntax::{TypeSyntax, parse_type_syntax},
        types::{Annotation, SurfaceType},
    },
    itertools::Itertools,
    ropey::Rope,
    serde::Deserialize,
    std::{num::NonZero, time::Instant},
    thiserror::Error,
    tree_sitter::{Node, TreeCursor},
};

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
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
        let error = tree::find_next_error(node).unwrap_or(node);

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

    let mut buffer = String::with_capacity(rope.len_bytes() * 3 / 2);
    traverse(
        &mut buffer,
        &mut node.walk(),
        Context {
            rope,
            indent: &" ".repeat(config.indent_width),
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
    let fmt_raw = |out: &mut String, node: Node| {
        out.push_str(&get_raw(node, context.rope));
        Ok(())
    };

    let node = cursor.node();
    let kind_id = node.kind_id();

    let unknown = |child: Node| -> Result<(), FormatError> {
        Err(FormatError::UnknownKind {
            kind: child.kind(),
            raw: get_raw(child, context.rope),
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

    if skipped_by_fmt_directive(node, context.rope) {
        // `# fmt: skip` preserves the node byte-exactly, including the column of its first line:
        // when only whitespace precedes the node on its line, the indentation the container just
        // emitted is replaced by the original leading whitespace, so column-aligned constructs
        // (aligned matrices, for instance) survive unchanged.
        let line_start = node.start_byte() - node.start_position().column;
        let leading = raw_between(context.rope, line_start, node.start_byte());
        if leading.chars().all(char::is_whitespace) {
            let content_length = out.trim_end_matches(' ').len();
            if content_length == 0 || out[..content_length].ends_with(context.line_ending) {
                out.truncate(content_length);
                out.push_str(&leading);
            }
        }
        return fmt_raw(out, node);
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
                // `#:` introduces a Roughly type annotation; format the whole annotation block.
                Some(':') => format_annotation_comment(out, node, context, level),
                Some(char @ ('\'' | '*')) => {
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
                // `!` is for shebangs (e.g. #!/usr/bin/env Rscript); `|` is for Quarto/knitr cell
                // options (e.g. #| echo: false), whose YAML payload must stay untouched.
                Some('#' | '!' | '|' | ' ') | None => out.push_str(raw),
                Some(other) => {
                    out.push_str("# ");
                    out.push(other);
                    out.push_str(&chars.collect::<String>());
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
            let node_text = get_raw(node, context.rope);
            // Raw string literals (R 4.0+: `r"(...)"`, `R"[...]"`, `r"---(...)---"`, etc.) start with
            // an `r`/`R` prefix; their delimiters and content must be preserved byte-for-byte, since
            // re-quoting would corrupt embedded quotes and backslashes (or, when tree-sitter does not
            // expose a content field, erase the literal entirely).
            if matches!(node_text.chars().next(), Some('r' | 'R')) {
                out.push_str(&node_text);
            } else if let Some(content) = field_optional(node, field::CONTENT) {
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
        kind::ESCAPE_SEQUENCE | kind::STRING_CONTENT => unknown(node)?,
        // KEYWORDS
        kind::DOTS => out.push_str("..."),
        kind::DOT_DOT_I => fmt_raw(out, node)?,
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                                        .child_count()
                                        .checked_sub(1)
                                        .and_then(|last_index| prev.child(last_index))
                                        .is_some_and(|last| last.kind_id() == kind::EQUAL)
                            }) {
                                space(out);
                            }
                            fmt(out, cursor)
                        }
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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

            let mut enabled = true;
            let mut maybe_directive = None;
            tree::for_each_child(cursor, |i, child, field_id, cursor| {
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

                if !enabled {
                    // A `# fmt: off` region is preserved byte-exactly (original line endings,
                    // blank lines, and columns); only directive comments themselves are formatted.
                    let start = maybe_prev.map_or(child.start_byte(), |prev| prev.end_byte());
                    return if maybe_directive.is_some() {
                        out.push_str(&raw_between(context.rope, start, child.start_byte()));
                        fmt(out, cursor)
                    } else {
                        out.push_str(&raw_between(context.rope, start, child.end_byte()));
                        Ok(())
                    };
                }

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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                    _ => unknown(child),
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
                    _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        kind::COMMENT if maybe_prev.is_some_and(|prev| same_line(prev, child)) => {
                            space(out);
                        }
                        _ => {
                            newlines(out, child, maybe_prev);
                        }
                    }
                    fmt(out, cursor)
                } else {
                    // A `# fmt: off` region is preserved byte-exactly (original line endings,
                    // blank lines, and columns); only directive comments themselves are formatted.
                    let start = maybe_prev.map_or(child.start_byte(), |prev| prev.end_byte());
                    if maybe_directive.is_some() {
                        out.push_str(&raw_between(context.rope, start, child.start_byte()));
                        fmt(out, cursor)
                    } else {
                        out.push_str(&raw_between(context.rope, start, child.end_byte()));
                        Ok(())
                    }
                }
            })?;

            // An empty file stays empty instead of gaining a newline (and thus being reported as
            // reformatted); a file that merely has no expressions keeps its single newline.
            if node.child_count() > 0 || context.rope.len_bytes() > 0 {
                newline(out);
            }
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
                        _ => unknown(child),
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
                            _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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
                        _ => unknown(child),
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

fn raw_between(rope: &Rope, start_byte: usize, end_byte: usize) -> String {
    rope.byte_slice(start_byte..end_byte).to_string()
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

// The grammar mis-reports the end of an extract operator (`x$y`) when a newline separates the operator
// from its right-hand side: the node's own end lands on the newline and excludes the right-hand side, so
// `node.end_position()` would place the extract's end before its `y`. Use the right-hand side's end (or
// the operator's, if the grammar detached the right-hand side entirely) to recover the true end.
fn end_position(node: Node) -> tree_sitter::Point {
    if node.kind_id() != kind::EXTRACT_OPERATOR {
        return node.end_position();
    }

    field_optional(node, field::RHS)
        .map(|rhs| rhs.end_position())
        .or_else(|| field_optional(node, field::OPERATOR).map(|operator| operator.end_position()))
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

// Whether a `# fmt: skip` directive exempts `node` from formatting: either a skip comment on its
// own line directly above the node, or a trailing skip comment on the node's last line.
fn skipped_by_fmt_directive(node: Node, rope: &Rope) -> bool {
    let is_fmt_skip_comment = |node: Node| {
        node.kind_id() == kind::COMMENT
            && parse_directive(&get_raw(node, rope))
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

    prev_is_fmt_skip || next_is_fmt_skip
}

//
// `#:` TYPE-ANNOTATION COMMENTS
//

// Formats the `#:` annotation block that starts at `node`; for a continuation line of a block the
// first comment already rendered, it removes the container's line separator and emits nothing.
//
// A block is a maximal run of sibling `#:` comments on adjacent rows — the same grouping the
// analysis uses when it attaches annotations. The joined block is parsed with the real annotation
// grammar (`analysis::type_syntax`); on success the whole block is re-rendered from its tokens (see
// `render_annotation_block`), and on failure every line is kept verbatim apart from ensuring a
// single space after `#:`, so prose and malformed annotations are never corrupted.
fn format_annotation_comment(out: &mut String, node: Node, context: Context, level: usize) {
    if is_annotation_continuation(node, context.rope) {
        // Rendering can merge, split, or drop block lines, so there is no fixed line-per-comment
        // mapping; the block's first comment emits everything and later comments must retract the
        // separator (line ending plus indentation) the container emitted for them.
        rewind_annotation_separator(out, context.line_ending);
        return;
    }

    let bodies = annotation_block_bodies(node, context.rope);
    let lines = render_annotation_block(&bodies, context.indent).unwrap_or_else(|| {
        bodies
            .iter()
            .map(|body| verbatim_annotation_line(body))
            .collect()
    });

    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push_str(context.line_ending);
            out.push_str(&context.indent.repeat(level));
        }
        out.push_str(line);
    }
}

// The body of a `#:` comment (the text after the marker, right-trimmed), or `None` for any other
// node.
fn annotation_body(node: Node, rope: &Rope) -> Option<String> {
    if node.kind_id() != kind::COMMENT {
        return None;
    }
    get_raw(node, rope)
        .trim_end()
        .strip_prefix("#:")
        .map(str::to_owned)
}

// Whether `node` is a non-first line of an annotation block: the previous sibling is a `#:` comment
// on the row directly above. A first line exempted by `# fmt: skip` is emitted raw and cannot render
// the block, so the block restarts at the line after it — and the same applies to a head that trails
// code on its own line (it is emitted inline at the code's position, so it cannot carry the block's
// continuation indentation; see `is_trailing_annotation_head`).
fn is_annotation_continuation(node: Node, rope: &Rope) -> bool {
    node.prev_sibling().is_some_and(|previous| {
        annotation_body(previous, rope).is_some()
            && node.start_position().row <= previous.end_position().row + 1
            && !skipped_by_fmt_directive(previous, rope)
            && !is_trailing_annotation_head(previous, rope)
    })
}

// Whether `node` is a `#:` comment that trails other code on its own line. Such a comment is
// emitted inline after the code, so it renders only itself; the `#:` lines below it form their own
// block at their own indentation.
fn is_trailing_annotation_head(node: Node, rope: &Rope) -> bool {
    annotation_body(node, rope).is_some()
        && node.prev_sibling().is_some_and(|previous| {
            previous.end_position().row == node.start_position().row
                && annotation_body(previous, rope).is_none()
        })
}

// The bodies of every line of the annotation block starting at `node`, in order. A head that trails
// code renders alone (its continuations restart as their own block).
fn annotation_block_bodies(node: Node, rope: &Rope) -> Vec<String> {
    let mut bodies = vec![annotation_body(node, rope).unwrap_or_default()];
    if is_trailing_annotation_head(node, rope) {
        return bodies;
    }
    let mut current = node;
    while let Some(next) = current.next_sibling() {
        let Some(body) = annotation_body(next, rope) else {
            break;
        };
        if next.start_position().row > current.end_position().row + 1 {
            break;
        }
        bodies.push(body);
        current = next;
    }
    bodies
}

// Retracts the line separator (line ending plus indentation) that the enclosing node emitted before
// a continuation comment. Best effort: if the buffer does not end in a separator — no current
// container produces that — it is left untouched; the block content was already rendered by the
// block's first line, so nothing is lost either way.
fn rewind_annotation_separator(out: &mut String, line_ending: &str) {
    if let Some(position) = out.rfind(line_ending)
        && out[position + line_ending.len()..]
            .bytes()
            .all(|byte| byte == b' ')
    {
        out.truncate(position);
    }
}

// A line of an annotation block that did not parse: verbatim, except that a single space is ensured
// after `#:`.
fn verbatim_annotation_line(body: &str) -> String {
    match body.chars().next() {
        None => "#:".to_owned(),
        Some(character) if character.is_whitespace() => format!("#:{body}"),
        Some(_) => format!("#: {body}"),
    }
}

// Renders a parse-valid annotation block as `#:` lines. The author's line breaks are preserved;
// everything else is re-derived:
//
// - Token spacing is canonical (the typing reference's rendered form): `,` and `:` hug their left
//   neighbor, `|` and `->` are surrounded by spaces, and brackets hug what they apply to.
// - Every bracket carries a hug bit read off the input. An opener followed by content on its own
//   line is hugged: its closer glues to the token before it (`@type F {list{` … `}}`). An opener
//   that ends its line is expanded: its closer gets its own line at the opener's line indent.
// - A content line is indented one `indent` step per enclosing expanded bracket, so both the fully
//   hugged and the fully expanded style are fixed points and mixed closer shapes normalize.
// - Trailing empty `#:` lines are dropped; leading and interior ones are kept.
//
// `None` when the joined block does not parse as annotation syntax (or a character falls outside
// the token alphabet, which the parse gate makes unlikely); the caller then emits the block
// verbatim.
fn render_annotation_block(bodies: &[String], indent: &str) -> Option<Vec<String>> {
    let joined = bodies.iter().map(|body| body.trim()).join("\n");
    let mut interner = Interner::new();
    match parse_type_syntax(&joined, &mut interner) {
        // Only a full, clean parse is trusted. `UnsupportedConstruct` is NOT admitted: the parser
        // can return it before consuming the whole input, so arbitrary prose after an unsupported
        // prefix would slip through and be reflowed as if it were type tokens.
        Ok(parsed) if annotation_parameter_names_are_plain(&parsed, &interner) => {}
        Ok(_) | Err(_) => return None,
    }
    let token_lines = bodies
        .iter()
        .map(|body| lex_annotation_line(body))
        .collect::<Option<Vec<_>>>()?;

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_level = 0;
    let mut open_delimiters: Vec<OpenDelimiter> = Vec::new();
    let mut previous_token: Option<&AnnotationToken> = None;
    let mut pending_empty_lines = 0;
    let mut started = false;

    for tokens in &token_lines {
        if tokens.is_empty() {
            if started {
                pending_empty_lines += 1;
            } else {
                lines.push("#:".to_owned());
            }
            continue;
        }

        for (index, token) in tokens.iter().enumerate() {
            let closer = is_closer(token.kind);
            let closes_expanded = closer
                && open_delimiters
                    .last()
                    .is_none_or(|delimiter| delimiter.expanded);

            let break_before = if !started {
                false
            } else if index == 0 && pending_empty_lines > 0 {
                // interior empty `#:` lines are author-intentional separators: the following
                // content always starts its own line, even a closer that would otherwise glue
                true
            } else if index == 0 {
                // an author line break is kept, unless a hugged bracket's closer glues back up
                !closer || closes_expanded
            } else {
                // an expanded bracket's closer never shares a line with the content before it
                closes_expanded
            };

            if break_before {
                lines.push(annotation_line(current_level, &current, indent));
                current.clear();
                for _ in 0..pending_empty_lines {
                    lines.push("#:".to_owned());
                }
                current_level = match open_delimiters.last() {
                    Some(delimiter) if closer => delimiter.opener_level,
                    Some(delimiter) => delimiter.opener_level + usize::from(delimiter.expanded),
                    None => 0,
                };
                previous_token = None;
            }
            pending_empty_lines = 0;

            if let Some(previous) = previous_token
                && annotation_space_between(previous, token)
            {
                current.push(' ');
            }
            current.push_str(token.text);
            previous_token = Some(token);
            started = true;

            if is_opener(token.kind) {
                open_delimiters.push(OpenDelimiter {
                    expanded: index + 1 == tokens.len(),
                    opener_level: current_level,
                });
            } else if closer {
                open_delimiters.pop();
            }
        }
    }

    if started {
        lines.push(annotation_line(current_level, &current, indent));
    }
    Some(lines)
}

// A bracket that is still open at the current point of the render walk. `expanded` is its hug bit
// (see `render_annotation_block`) and `opener_level` the indent level of the line its opener sits
// on, which is where an expanded closer's own line goes.
struct OpenDelimiter {
    expanded: bool,
    opener_level: usize,
}

// Guards the render gate against parameter names that are not plain identifiers. The parser
// validates `@param name {TYPE}` names strictly, so this is defense in depth for any grammar
// path that still interns a looser name; a block whose function-parameter names are not plain
// identifiers is emitted verbatim rather than reflowed.
fn annotation_parameter_names_are_plain(parsed: &TypeSyntax, interner: &Interner) -> bool {
    fn is_plain_name(name: &str) -> bool {
        // The optional-name form `[ label ]` may intern surrounding padding with the name; padding
        // is not prose, so it is trimmed before the shape check.
        let name = name.trim();
        let mut characters = name.chars();
        characters
            .next()
            .is_some_and(|first| first.is_alphabetic() || first == '_' || first == '.')
            && characters.all(|character| {
                character.is_alphanumeric() || character == '_' || character == '.'
            })
    }

    fn surface_type_is_plain(surface_type: &SurfaceType, interner: &Interner) -> bool {
        match surface_type {
            SurfaceType::Function(function_type) => {
                function_type.named_parameters.iter().all(|parameter| {
                    interner.resolve(parameter.name).is_some_and(is_plain_name)
                        && surface_type_is_plain(&parameter.value, interner)
                }) && function_type
                    .parameters
                    .iter()
                    .all(|parameter| surface_type_is_plain(parameter, interner))
                    && function_type
                        .variadic
                        .as_ref()
                        .is_none_or(|variadic| surface_type_is_plain(&variadic.element, interner))
                    && surface_type_is_plain(&function_type.return_type, interner)
            }
            SurfaceType::Union(members) | SurfaceType::Tuple(members) => members
                .iter()
                .all(|member| surface_type_is_plain(member, interner)),
            SurfaceType::Named(_, arguments) => arguments
                .iter()
                .all(|argument| surface_type_is_plain(argument, interner)),
            SurfaceType::Record(fields) => fields
                .iter()
                .all(|field| surface_type_is_plain(&field.value, interner)),
            SurfaceType::Vector(inner)
            | SurfaceType::NamedVector(inner)
            | SurfaceType::List(inner)
            | SurfaceType::NamedList(inner)
            | SurfaceType::Binders(_, inner) => surface_type_is_plain(inner, interner),
            SurfaceType::Any
            | SurfaceType::Unknown
            | SurfaceType::Null
            | SurfaceType::Scalar(_) => true,
        }
    }

    fn annotation_is_plain(annotation: &Annotation, interner: &Interner) -> bool {
        match annotation {
            Annotation::Type { surface_type, .. } => surface_type_is_plain(surface_type, interner),
            Annotation::New { .. } => true,
        }
    }

    match parsed {
        TypeSyntax::Annotation(annotation) => annotation_is_plain(annotation, interner),
        TypeSyntax::Definitions(_) => true,
    }
}

fn annotation_line(level: usize, content: &str, indent: &str) -> String {
    format!("#: {}{content}", indent.repeat(level))
}

// Whether a single space separates two adjacent annotation tokens, reproducing the canonical
// surface-type spacing (the same form `analysis::type_syntax::render_surface_type` produces).
// (A vector suffix after `}` never reaches here: `list{...}[]` is an unsupported construct, and
// unsupported constructs go down the verbatim path.)
fn annotation_space_between(previous: &AnnotationToken, current: &AnnotationToken) -> bool {
    use AnnotationTokenKind::*;

    match current.kind {
        // separators and closers hug their left neighbor
        Comma | Colon | CloseParen | CloseBracket | CloseBrace | CloseAngle => return false,
        // brackets hug what they apply to: `fn(`, `Foo<`, `list[`/`T[]`, `list{`
        OpenParen => return false,
        OpenAngle if previous.kind == Word => return false,
        OpenBracket if matches!(previous.kind, Word | CloseAngle | CloseBracket) => return false,
        OpenBrace if previous.kind == Word && previous.text == "list" => return false,
        // a named rest parameter keeps its name attached: `...args`
        Word if previous.kind == Dots => return false,
        _ => {}
    }

    // nothing follows an opening bracket with a space
    !matches!(
        previous.kind,
        OpenParen | OpenBracket | OpenBrace | OpenAngle
    )
}

// Lexes one annotation line over the surface-type token alphabet. `None` on any character outside
// it, which sends the block down the verbatim path.
fn lex_annotation_line(body: &str) -> Option<Vec<AnnotationToken<'_>>> {
    use AnnotationTokenKind::*;

    let mut tokens = Vec::new();
    let mut position = 0;
    while position < body.len() {
        let remaining = &body[position..];
        let character = remaining.chars().next()?;
        if character.is_whitespace() {
            position += character.len_utf8();
            continue;
        }

        let (kind, length) = match character {
            '(' => (OpenParen, 1),
            ')' => (CloseParen, 1),
            '[' => (OpenBracket, 1),
            ']' => (CloseBracket, 1),
            '{' => (OpenBrace, 1),
            '}' => (CloseBrace, 1),
            '<' => (OpenAngle, 1),
            '>' => (CloseAngle, 1),
            ',' => (Comma, 1),
            ':' => (Colon, 1),
            '|' => (Pipe, 1),
            '-' if remaining.starts_with("->") => (Arrow, 2),
            '.' if remaining.starts_with("...") => (Dots, 3),
            '@' => {
                let length = remaining[1..]
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                    .map_or(remaining.len(), |end| end + 1);
                (Directive, length)
            }
            // the grammar's identifiers are unicode-alphabetic, and member names admit interior dots
            c if c == '_' || c.is_alphabetic() => {
                let length = remaining
                    .find(|c: char| !(c == '_' || c == '.' || c.is_alphanumeric()))
                    .unwrap_or(remaining.len());
                (Word, length)
            }
            _ => return None,
        };
        tokens.push(AnnotationToken {
            kind,
            text: &remaining[..length],
        });
        position += length;
    }
    Some(tokens)
}

fn is_opener(kind: AnnotationTokenKind) -> bool {
    use AnnotationTokenKind::*;
    matches!(kind, OpenParen | OpenBracket | OpenBrace | OpenAngle)
}

fn is_closer(kind: AnnotationTokenKind) -> bool {
    use AnnotationTokenKind::*;
    matches!(kind, CloseParen | CloseBracket | CloseBrace | CloseAngle)
}

struct AnnotationToken<'a> {
    kind: AnnotationTokenKind,
    text: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnotationTokenKind {
    Word,
    Directive,
    Dots,
    Arrow,
    Pipe,
    Comma,
    Colon,
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    OpenBrace,
    CloseBrace,
    OpenAngle,
    CloseAngle,
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
