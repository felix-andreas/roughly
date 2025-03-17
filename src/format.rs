use {
    crate::{cli, config, tree, utils},
    console::style,
    ignore::Walk,
    itertools::Itertools,
    ropey::Rope,
    std::{path::PathBuf, time::Instant},
    thiserror::Error,
    tree_sitter::{Node, TreeCursor},
};

#[derive(Debug, Clone, Copy)]
pub struct Config<'a> {
    pub indent: &'a str,
    pub line_ending: LineEnding,
    pub stop_on_unhandled_comment: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum LineEnding {
    Auto,
    Lf,
    Crlf,
}

#[derive(Debug)]
pub struct FormatRunError;

pub fn run(
    maybe_files: Option<&[PathBuf]>,
    check: bool,
    diff: bool,
    stop_on_unhandled_comment: bool,
) -> Result<(), FormatRunError> {
    let root: Vec<PathBuf> = vec![".".into()];
    let files = maybe_files.unwrap_or(&root);

    let paths_with_config = files
        .iter()
        .map(|file| {
            let config = match config::Config::from_path(file) {
                Ok(config) => config,
                Err(error) => {
                    cli::error(&error.to_string());
                    return Err(FormatRunError);
                }
            };

            let paths = Walk::new(file)
                .filter_map(|entry| match entry {
                    Ok(entry) => {
                        let path = entry.into_path();
                        path.extension()
                            .is_some_and(|ext| ext == "R" || ext == "r")
                            .then_some(Ok(path))
                    }
                    Err(error) => {
                        cli::error(&error.to_string());
                        Some(Err(FormatRunError))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;

            Ok((paths, config))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut n_files = 0;
    let mut n_unformatted = 0;
    let mut n_errors = 0;
    for (paths, config) in paths_with_config {
        let config = Config {
            indent: &" ".repeat(config.spaces),
            line_ending: LineEnding::Auto,
            stop_on_unhandled_comment,
        };
        for path in paths {
            n_files += 1;
            let old = match std::fs::read_to_string(&path) {
                Ok(old) => old,
                Err(err) => {
                    n_errors += 1;
                    cli::error(&format!("failed to format: {}", path.display()));
                    eprintln!("{err}");
                    continue;
                }
            };
            let tree = tree::parse(&old, None);
            let rope = Rope::from_str(&old);
            let new = match format(tree.root_node(), &rope, config) {
                Ok(new) => new,
                Err(err) => {
                    n_errors += 1;
                    cli::error(&format!("failed to format: {}", path.display()));
                    eprintln!("{err}");
                    continue;
                }
            };
            if old != new {
                n_unformatted += 1;
                if diff {
                    eprintln!("Diff in {}:", path.display());
                    utils::print_diff(&old, &new);
                } else if check {
                    eprintln!("Would reformat: {}", style(path.display()).bold());
                } else if std::fs::write(&path, &new).is_err() {
                    cli::error(&format!("failed to write to file: {}", path.display()));
                }
            }
        }
    }

    if n_files == 0 {
        cli::warning("No R files found under the given path(s)");
        return Err(FormatRunError);
    }

    let (action_format, action_skip) = if check || diff {
        ("would be reformatted", "already formatted")
    } else {
        ("reformatted", "left unchanged")
    };

    let n_unchanged = n_files - n_unformatted;
    cli::info(&format!(
        "{} file{} {}, {} file{} {}",
        n_unformatted,
        match n_unformatted {
            1 => "",
            _ => "s",
        },
        action_format,
        n_unchanged,
        match n_unchanged {
            1 => "",
            _ => "s",
        },
        action_skip
    ));

    if n_unformatted == 0 && n_errors == 0 {
        Ok(())
    } else {
        Err(FormatRunError)
    }
}

#[derive(Error, Debug)]
pub enum FormatError {
    #[error("Unexpected {kind} at line {line} col {col}")]
    SyntaxError {
        kind: &'static str,
        line: usize,
        col: usize,
    },
    #[error("Missing {kind} at line {line} col {col}")]
    Missing {
        kind: &'static str,
        line: usize,
        col: usize,
    },
    #[error("The node has unknown type {kind}: {raw}")]
    Unknown { kind: &'static str, raw: String },
    #[error("Unhandled comment in node at {line}:{col}: \"{node}\"")]
    UnhandledComment {
        node: String,
        line: usize,
        col: usize,
    },
    #[error("Missing field {field} for node of kind {kind}")]
    MissingField {
        kind: &'static str,
        field: &'static str,
    },
}

pub fn format(node: Node, rope: &Rope, config: Config) -> Result<String, FormatError> {
    let start = Instant::now();
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

    let formatted = {
        if node.has_error() {
            let error = tree::find_next_error(node).unwrap();
            return Err(if error.is_missing() {
                FormatError::Missing {
                    kind: error.kind(),
                    line: error.start_position().row,
                    col: error.start_position().column,
                }
            } else {
                FormatError::SyntaxError {
                    kind: error.kind(),
                    line: error.start_position().row,
                    col: error.start_position().column,
                }
            });
        }

        let mut buffer = String::with_capacity(rope.len_bytes() * 3 / 2);
        traverse(
            &mut node.walk(),
            &mut buffer,
            ConfigHelper {
                rope,
                indent: config.indent,
                line_ending,
                stop_on_unhandled_comment: config.stop_on_unhandled_comment,
            },
            0,
            false,
        )?;
        buffer
    };

    let elapsed = start.elapsed();
    log::debug!(
        "formatted {} lines in {} ms ({}/s)",
        rope.len_lines(),
        elapsed.as_millis(),
        utils::human_bytes(rope.len_bytes() as f64 / elapsed.as_secs_f64())
    );
    Ok(formatted)
}

#[derive(Debug, Clone, Copy)]
struct ConfigHelper<'a> {
    rope: &'a Rope,
    indent: &'a str,
    line_ending: &'static str,
    stop_on_unhandled_comment: bool,
}

fn traverse(
    cursor: &mut TreeCursor,
    out: &mut String,
    config: ConfigHelper,
    level: usize,
    make_multiline: bool,
) -> Result<(), FormatError> {
    let space = |out: &mut String| out.push(' ');
    let indent = |out: &mut String| out.push_str(config.indent);
    let newline = |out: &mut String| {
        out.push_str(config.line_ending);
        out.push_str(&config.indent.repeat(level));
    };
    let newlines = |out: &mut String, node: Node, maybe_prev: Option<Node>| {
        out.push_str(&config.line_ending.repeat(maybe_prev.map_or(0, |prev| {
            usize::clamp(node.start_position().row - prev.end_position().row, 1, 2)
        })));
        out.push_str(&config.indent.repeat(level));
    };

    let fmt_with =
        |cursor: &mut TreeCursor, out: &mut String, level: usize, make_multline: bool| {
            traverse(cursor, out, config, level, make_multline)
        };

    let fmt = |cursor: &mut TreeCursor, out: &mut String| fmt_with(cursor, out, level, false);
    let fmt_indent = |cursor: &mut TreeCursor, out: &mut String| {
        indent(out);
        fmt_with(cursor, out, level + 1, false)
    };
    let fmt_indent_skip_first =
        |cursor: &mut TreeCursor, out: &mut String| fmt_with(cursor, out, level + 1, false);
    let fmt_multiline =
        |cursor: &mut TreeCursor, out: &mut String| fmt_with(cursor, out, level, true);
    let fmt_braces = |cursor: &mut TreeCursor, out: &mut String| {
        out.push('{');
        newline(out);
        fmt_indent(cursor, out)?;

        newline(out);
        out.push('}');
        Ok(())
    };

    let get_raw = |node: Node| config.rope.byte_slice(node.byte_range()).to_string();
    let fmt_raw = |node: Node, out: &mut String| {
        out.push_str(&get_raw(node));
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

        node.child_by_field_name("rhs")
            .map(|rhs| rhs.end_position())
            .or_else(|| {
                node.child_by_field_name("operator")
                    .map(|operator| operator.end_position())
            })
            // note: this case is unexpected
            .unwrap_or_else(|| node.end_position())
    };
    let same_line = |a: Node, b: Node| end_position(a).row == b.start_position().row;
    let is_fmt_skip_comment =
        |node: Node| node.kind() == "comment" && get_raw(node).contains("fmt: skip");

    let missing = |node: Node| FormatError::Missing {
        kind: node.kind(),
        line: node.start_position().row,
        col: node.start_position().column,
    };
    let syntax_error = |node: Node| FormatError::SyntaxError {
        kind: node.kind(),
        line: node.start_position().row,
        col: node.start_position().column,
    };

    let node = cursor.node();
    let kind = node.kind();
    let push_all_comments = |cursor: &mut TreeCursor, out: &mut String| {
        let is_first = true;
        tree::for_each_child(cursor, |_, child, _, cursor| {
            if child.kind() == "comment" {
                if is_first && node.prev_sibling().is_some() {
                    newline(out);
                }
                fmt(cursor, out)?;
                newline(out);
            }
            Ok(())
        })
    };

    // note: currently we don't traverse open & close -> they never reach these conditions
    if node.is_error() {
        return Err(syntax_error(node));
    }

    if node.is_missing() {
        return Err(missing(node));
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

            let _ = chars.next();
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

            push_all_comments(cursor, out)?;

            tree::for_each_child(cursor, |_, child, field_name, cursor| match field_name {
                None => match child.kind() {
                    "comment" => Ok(()),
                    "=" => {
                        out.push_str(" = ");
                        Ok(())
                    }
                    _ => unreachable!(),
                },
                Some(field_name) => match field_name {
                    "name" => fmt(cursor, out),
                    "value" | "default" => fmt(cursor, out),
                    _ => unreachable!(),
                },
            })?;
        }
        "arguments" | "parameters" => {
            handles_comments = true;

            let is_multiline = !same_line(node, node);
            let is_empty = node.child_count() == 2;
            let hug = node.child_count() == 3
                && node
                    .child_by_field_name("argument")
                    .is_some_and(|argument| {
                        argument.child_count() == 1
                            && argument.child(0).unwrap().kind() == "braced_expression"
                    });

            tree::for_each_child(cursor, |i, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                space(out);
                                fmt(cursor, out)
                            } else {
                                newlines(out, child, maybe_prev);
                                fmt_indent(cursor, out)
                            }
                        }
                        "comma" => {
                            if is_multiline
                                && (maybe_prev
                                    .is_none_or(|node| ["comment", "comma"].contains(&node.kind()))
                                    || i == 1)
                            {
                                newline(out);
                                indent(out);
                            }
                            fmt(cursor, out)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => fmt(cursor, out),
                        "argument" | "parameter" => {
                            if i == 1 {
                                if is_multiline && !hug {
                                    newline(out);
                                }
                            } else if is_multiline {
                                newlines(out, child, maybe_prev);
                            } else {
                                space(out);
                            }

                            if is_multiline && !hug {
                                fmt_indent(cursor, out)
                            } else {
                                fmt(cursor, out)
                            }
                        }
                        "close" => {
                            if is_multiline && !hug && !is_empty {
                                newline(out);
                            }
                            fmt(cursor, out)
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        "binary_operator" => {
            handles_comments = true;

            let is_multiline = !same_line(field(node, "lhs")?, field(node, "rhs")?);
            let operator = field(node, "operator")?;
            let has_spacing = !(operator.kind() == ":" || operator.kind() == "^");

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                space(out);
                                fmt(cursor, out)
                            } else {
                                newline(out);
                                fmt_indent(cursor, out)
                            }
                        }
                        _ => {
                            unreachable!()
                        }
                    },
                    Some(field_name) => match field_name {
                        "lhs" => fmt(cursor, out),
                        "operator" => {
                            if has_spacing {
                                space(out);
                            }
                            fmt(cursor, out)
                        }
                        "rhs" => {
                            if is_multiline {
                                newline(out);
                                fmt_indent(cursor, out)
                            } else {
                                if has_spacing {
                                    space(out)
                                }
                                fmt(cursor, out)
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            })?;
        }
        "braced_expression" => {
            handles_comments = true;

            let is_multiline = !same_line(node, node) || make_multiline;
            let is_empty = node.child_count() == 2;

            tree::for_each_child(cursor, |i, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                space(out);
                                fmt(cursor, out)
                            } else {
                                newlines(out, child, maybe_prev);
                                fmt_indent(cursor, out)
                            }
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => fmt(cursor, out),
                        "body" => {
                            if is_multiline {
                                if i == 1 {
                                    newline(out)
                                } else {
                                    newlines(out, child, maybe_prev);
                                }
                                fmt_indent(cursor, out)
                            } else {
                                if i == 1 {
                                    space(out)
                                } else {
                                    out.push_str("; ");
                                }
                                fmt(cursor, out)
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
                            fmt(cursor, out)
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
                        if let Some(prev) = maybe_prev
                            && same_line(prev, child)
                        {
                            space(out);
                            fmt(cursor, out)
                        } else {
                            newline(out);
                            fmt_indent(cursor, out)
                        }
                    }
                    _ => unreachable!(),
                },
                Some(field_name) => match field_name {
                    "function" => fmt(cursor, out),
                    "arguments" => {
                        if additional_indent {
                            fmt_indent_skip_first(cursor, out)
                        } else {
                            fmt(cursor, out)
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
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                space(out);
                                fmt(cursor, out)
                            } else {
                                newline(out);
                                fmt_indent(cursor, out)
                            }
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "lhs" => fmt(cursor, out),
                        "operator" => fmt(cursor, out),
                        "rhs" => {
                            if is_multiline {
                                newline(out);
                                fmt_indent(cursor, out)
                            } else {
                                fmt(cursor, out)
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

            let condition_is_multiline = !same_line(field(node, "open")?, field(node, "close")?);
            let loop_header_is_multiline =
                !same_line(field(node, "variable")?, field(node, "sequence")?);

            let mut indent_comments = false;
            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "for" => fmt(cursor, out),
                        "in" => {
                            if prev_is_comment || loop_header_is_multiline {
                                newline(out);
                                fmt_indent(cursor, out)
                            } else {
                                space(out);
                                fmt(cursor, out)
                            }
                        }
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                space(out);
                            } else {
                                newline(out);
                                if indent_comments {
                                    indent(out);
                                }
                            }
                            fmt(cursor, out)
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
                            fmt(cursor, out)
                        }
                        "variable" => {
                            if prev_is_comment || condition_is_multiline {
                                newline(out);
                                fmt_indent(cursor, out)
                            } else {
                                fmt(cursor, out)
                            }
                        }
                        "sequence" => {
                            if prev_is_comment {
                                newline(out);
                                fmt_indent(cursor, out)
                            } else {
                                space(out);
                                if condition_is_multiline {
                                    fmt_indent_skip_first(cursor, out)
                                } else {
                                    fmt(cursor, out)
                                }
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            if prev_is_comment || condition_is_multiline {
                                newline(out);
                            }
                            fmt(cursor, out)
                        }
                        "body" => {
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out)
                            }
                            if child.kind() == "braced_expression" {
                                fmt_multiline(cursor, out)
                            } else {
                                fmt_braces(cursor, out)
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

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                space(out);
                            } else {
                                newline(out);
                            }
                            fmt(cursor, out)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "name" => fmt(cursor, out),
                        "parameters" => {
                            if prev_is_comment {
                                newline(out);
                            }
                            fmt(cursor, out)
                        }
                        "body" => {
                            if prev_is_comment {
                                newline(out)
                            } else {
                                space(out)
                            }
                            if is_multiline {
                                if child.kind() == "braced_expression" {
                                    fmt_multiline(cursor, out)
                                } else {
                                    fmt_braces(cursor, out)
                                }
                            } else {
                                fmt(cursor, out)
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
                let is_braced_without_comments = condition.kind() == "braced_expression"
                    && !is_comment(condition.prev_sibling())
                    && !is_comment(condition.next_sibling());
                let condition_is_multiline =
                    !same_line(field(node, "open")?, field(node, "close")?);
                !condition_is_multiline || is_braced_without_comments
            };

            let mut indent_comments = false;
            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "if" => fmt(cursor, out),
                        "else" => {
                            if prev_is_comment {
                                newline(out);
                            } else {
                                space(out);
                            }
                            fmt(cursor, out)
                        }
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                space(out);
                            } else {
                                newline(out);
                                if indent_comments {
                                    indent(out);
                                }
                            }
                            fmt(cursor, out)
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
                            fmt(cursor, out)
                        }
                        "condition" => {
                            if !hug {
                                newline(out);
                                fmt_indent(cursor, out)
                            } else {
                                fmt(cursor, out)
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            if !hug {
                                newline(out);
                            }
                            fmt(cursor, out)
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
                                    fmt_multiline(cursor, out)
                                } else {
                                    fmt_braces(cursor, out)
                                }
                            } else {
                                fmt(cursor, out)
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

            let is_multiline = !same_line(node, node);
            let is_empty = node.child_count() == 2;

            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                space(out);
                            } else {
                                newline(out);
                                out.push_str(config.indent);
                            }
                            fmt(cursor, out)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => fmt(cursor, out),
                        "body" => {
                            if is_multiline {
                                newline(out)
                            }

                            if is_multiline {
                                fmt_indent(cursor, out)
                            } else {
                                fmt(cursor, out)
                            }
                        }
                        "close" => {
                            if is_multiline && !is_empty {
                                newline(out);
                            }
                            fmt(cursor, out)
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
                fmt(cursor, out)
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
                        "repeat" => fmt(cursor, out),
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                space(out);
                            } else {
                                newline(out);
                            }
                            fmt(cursor, out)
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
                                    fmt_multiline(cursor, out)
                                } else {
                                    fmt_braces(cursor, out)
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            })?;
        }
        "string" => {
            out.push('"');
            tree::for_each_child(cursor, |_, child, field_name, cursor| match field_name {
                None => match child.kind() {
                    "\"" => Ok(()),
                    "\'" => Ok(()),
                    _ => unreachable!(),
                },
                Some("content") => fmt(cursor, out),
                Some(_) => unreachable!(),
            })?;
            out.push('"');
        }
        "string_content" => {
            let mut last_was_escape = false;
            for char in get_raw(node).chars() {
                match char {
                    '"' if !last_was_escape => out.push_str("\\\""),
                    _ => out.push(char),
                }
                last_was_escape = char == '\\' && !last_was_escape;
            }
        }
        "unary_operator" => {
            handles_comments = true;

            let rhs = field(node, "rhs")?;
            let operator = field(node, "operator")?;

            let has_space = operator.kind() == "~" && rhs.kind() != "identifer";

            push_all_comments(cursor, out)?;
            tree::for_each_child(
                &mut node.walk(),
                |_, child, field_name, cursor| match field_name {
                    None => match child.kind() {
                        "comment" => Ok(()),
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "operator" => fmt(cursor, out),
                        "rhs" => {
                            if has_space {
                                space(out);
                            }
                            fmt(cursor, out)
                        }
                        _ => unreachable!(),
                    },
                },
            )?;
        }
        "while_statement" => {
            handles_comments = true;

            let hug = {
                let condition = field(node, "condition")?;
                let is_braced_without_comments = condition.kind() == "braced_expression"
                    && !is_comment(condition.prev_sibling())
                    && !is_comment(condition.next_sibling());
                let condition_is_multiline =
                    !same_line(field(node, "open")?, field(node, "close")?);
                !condition_is_multiline || is_braced_without_comments
            };

            let mut indent_comments = false;
            tree::for_each_child(cursor, |_, child, field_name, cursor| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "while" => fmt(cursor, out),
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                space(out);
                            } else {
                                newline(out);
                                if indent_comments {
                                    indent(out);
                                }
                            }
                            fmt(cursor, out)
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
                            fmt(cursor, out)
                        }
                        "condition" => {
                            if !hug {
                                newline(out);
                                fmt_indent(cursor, out)
                            } else {
                                fmt(cursor, out)
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            if !hug {
                                newline(out);
                            }
                            fmt(cursor, out)
                        }
                        "body" => {
                            if prev_is_comment {
                                newline(out)
                            } else {
                                space(out)
                            }
                            if child.kind() == "braced_expression" {
                                fmt_multiline(cursor, out)
                            } else {
                                fmt_braces(cursor, out)
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
            log::error!(
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
        push_all_comments(cursor, out)?;

        if config.stop_on_unhandled_comment && out.len() != before {
            let start = node.start_position();
            return Err(FormatError::UnhandledComment {
                node: get_raw(node),
                line: start.row,
                col: start.column,
            });
        }
    }
    Ok(())
}
