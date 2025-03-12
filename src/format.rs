use {
    crate::{cli, config, tree, utils},
    console::style,
    ignore::Walk,
    itertools::Itertools,
    ropey::Rope,
    std::{path::PathBuf, time::Instant},
    thiserror::Error,
    tree_sitter::Node,
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
    #[error("Unhandled comment in line {line}:{col}: \"{comment}\"")]
    UnhandledComment {
        comment: String,
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
        let mut buffer = String::with_capacity(rope.len_bytes() * 3 / 2);
        traverse(
            node,
            rope,
            line_ending,
            config,
            State::default(),
            &mut buffer,
        )?;
        utils::remove_indent_prefix(&buffer)
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

#[derive(Debug, Clone, Copy, Default)]
struct State {
    make_multiline: bool,
}

impl State {
    fn make_multiline(make_multiline: bool) -> Self {
        State { make_multiline }
    }
}

fn traverse(
    node: Node,
    rope: &Rope,
    line_ending: &'static str,
    config: Config,
    state: State,
    out: &mut String,
) -> Result<(), FormatError> {
    // enum Fmt {
    //     Normal,
    //     Raw,
    //     WrapWithBraces,
    //     WithIndentPrefix,
    //     MakeMultiline,
    // }
    // let fmt_with = |node: Node, action: Fmt| match action {
    //     Fmt::Normal => fmt(node),
    //     Fmt::Raw => Ok(fmt_raw(node)),
    //     Fmt::WrapWithBraces => fmt_wrap_with_braces(node),
    //     Fmt::WithIndentPrefix => Ok(fmt_with_indent_prefix(node)),
    //     Fmt::MakeMultiline => fmt_multiline(node, state.make_multiline),
    // };

    let kind = node.kind();
    let fmt = |node: Node, out: &mut String| -> Result<(), FormatError> {
        traverse(node, rope, line_ending, config, State::default(), out)
    };
    let get_raw = |node: Node| rope.byte_slice(node.byte_range()).to_string();
    let fmt_raw = |node: Node, out: &mut String| {
        out.push_str(&get_raw(node));
        Ok(())
    };
    let fmt_multiline =
        |node: Node, make_multiline: bool, out: &mut String| -> Result<(), FormatError> {
            traverse(
                node,
                rope,
                line_ending,
                config,
                State::make_multiline(make_multiline),
                out,
            )
        };
    let fmt_with_indent_prefix = |node: Node, out: &mut String| -> Result<(), FormatError> {
        out.push_str(&utils::add_indent_prefix(&get_raw(node)));
        Ok(())
    };
    let fmt_wrap_with_braces = |node: Node, out: &mut String| -> Result<(), FormatError> {
        out.push('{');
        out.push_str(line_ending);

        let mut inner = String::new();
        fmt(node, &mut inner)?;
        out.push_str(&utils::indent_by(config.indent, &inner, line_ending));

        out.push_str(line_ending);
        out.push('}');
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

    let collapse_newlines = |node: Node, maybe_prev: Option<Node>| {
        line_ending.repeat(maybe_prev.map_or(0, |prev| {
            usize::clamp(node.start_position().row - prev.end_position().row, 1, 2)
        }))
    };
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
    let push_all_comments = |node: Node, out: &mut String| -> Result<(), _> {
        let is_first = true;
        tree::for_each_child::<FormatError>(&mut node.walk(), |_, child, _| {
            if child.kind() == "comment" {
                if is_first {
                    out.push_str(line_ending);
                }
                fmt(child, out)?;
                out.push_str(line_ending);
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
            return fmt_with_indent_prefix(node, out);
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

            let mut maybe_lhs = None;
            let mut maybe_rhs = None;
            let mut has_equal = false;
            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            fmt(child, out)?;
                            out.push_str(line_ending);
                        }
                        "=" => {
                            has_equal = true;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "name" => maybe_lhs = Some(child),
                        "value" | "default" => maybe_rhs = Some(child),
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;

            if let Some(name) = maybe_lhs {
                fmt(name, out)?;
                if has_equal {
                    out.push(' ');
                }
            }

            if has_equal {
                out.push_str("= ");
            }

            if let Some(value) = maybe_rhs {
                fmt(value, out)?;
            }
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

            tree::for_each_child(&mut node.walk(), |i, child, field_name| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(&collapse_newlines(child, maybe_prev));
                                out.push_str(config.indent);
                            }
                            fmt(child, out)?;
                        }
                        "comma" => {
                            if is_multiline
                                && (maybe_prev
                                    .is_none_or(|node| ["comment", "comma"].contains(&node.kind()))
                                    || i == 1)
                            {
                                out.push_str(line_ending);
                                out.push_str(config.indent);
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            fmt(child, out)?;
                        }
                        "argument" | "parameter" => {
                            if i == 1 {
                                if is_multiline && !hug {
                                    out.push_str(line_ending);
                                }
                            } else {
                                out.push_str(&if is_multiline {
                                    collapse_newlines(child, maybe_prev)
                                } else {
                                    " ".into()
                                });
                            }

                            if is_multiline && !hug {
                                let mut temp = String::new();
                                fmt(child, &mut temp)?;
                                out.push_str(&utils::indent_by(config.indent, &temp, line_ending));
                            } else {
                                fmt(child, out)?;
                            }
                        }
                        "close" => {
                            if is_multiline && !hug && !is_empty {
                                out.push_str(line_ending);
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
        }
        "binary_operator" => {
            handles_comments = true;

            let is_multiline = !same_line(field(node, "lhs")?, field(node, "rhs")?);
            let has_spacing = field(node, "operator")?.kind() == ":";

            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(line_ending);
                                out.push_str(config.indent);
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "lhs" => {
                            fmt(child, out)?;
                        }
                        "operator" => {
                            if !has_spacing {
                                out.push(' ');
                            }
                            fmt(child, out)?;
                        }
                        "rhs" => {
                            if is_multiline {
                                out.push_str(line_ending);

                                let mut temp = String::new();
                                fmt(child, &mut temp)?;
                                out.push_str(&utils::indent_by(config.indent, &temp, line_ending));
                            } else {
                                if !has_spacing {
                                    out.push(' ');
                                }
                                fmt(child, out)?;
                            }
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
        }
        "braced_expression" => {
            handles_comments = true;

            let is_multiline = !same_line(node, node) || state.make_multiline;
            let is_empty = node.child_count() == 2;

            tree::for_each_child(&mut node.walk(), |i, child, field_name| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(&collapse_newlines(child, maybe_prev));
                                out.push_str(config.indent);
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            fmt(child, out)?;
                        }
                        "body" => {
                            if i == 1 {
                                out.push_str(if is_multiline { line_ending } else { " " });
                            } else {
                                out.push_str(&if is_multiline {
                                    collapse_newlines(child, maybe_prev)
                                } else {
                                    "; ".into()
                                });
                            }

                            if is_multiline {
                                let mut temp = String::new();
                                fmt(child, &mut temp)?;
                                out.push_str(&utils::indent_by(config.indent, &temp, line_ending));
                            } else {
                                fmt(child, out)?;
                            }
                        }
                        "close" => {
                            if !is_empty {
                                out.push_str(if is_multiline { line_ending } else { " " });
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
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

            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            let maybe_prev = child.prev_sibling();
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(line_ending);
                                out.push_str(config.indent);
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "function" => {
                            fmt(child, out)?;
                        }
                        "arguments" => {
                            if additional_indent {
                                let mut temp = String::new();
                                fmt(child, &mut temp)?;
                                out.push_str(&utils::indent_by_skip_first(
                                    config.indent,
                                    &temp,
                                    line_ending,
                                ));
                            } else {
                                fmt(child, out)?;
                            }
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
        }
        "complex" => fmt_raw(node, out)?,
        "extract_operator" | "namespace_operator" => {
            handles_comments = true;

            let lhs = field(node, "lhs")?;
            let is_multiline = field_optional(node, "rhs").is_some_and(|rhs| !same_line(lhs, rhs));

            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(line_ending);
                                out.push_str(config.indent);
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "lhs" => {
                            fmt(child, out)?;
                        }
                        "operator" => {
                            fmt(child, out)?;
                        }
                        "rhs" => {
                            if is_multiline {
                                out.push_str(line_ending);

                                let mut temp = String::new();
                                fmt(child, &mut temp)?;
                                out.push_str(&utils::indent_by(config.indent, &temp, line_ending));
                            } else {
                                fmt(child, out)?;
                            }
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
        }
        "float" => fmt_raw(node, out)?,
        "for_statement" => {
            handles_comments = true;

            let condition_is_multiline = !same_line(field(node, "open")?, field(node, "close")?);
            let loop_header_is_multiline =
                !same_line(field(node, "variable")?, field(node, "sequence")?);

            let mut indent_comments = false;
            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();

                let prev_is_comment = is_comment(maybe_prev);
                let next_is_comment = is_comment(child.next_sibling());

                match field_name {
                    None => match child.kind() {
                        "for" => fmt(child, out)?,
                        "in" => {
                            if prev_is_comment {
                                out.push_str(config.indent);
                            } else if loop_header_is_multiline {
                                out.push_str(line_ending);
                                out.push_str(config.indent);
                            } else {
                                out.push(' ');
                            }
                            fmt(child, out)?;
                            if !next_is_comment {
                                out.push(' ');
                            }
                        }
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                if !prev_is_comment {
                                    out.push_str(line_ending);
                                }
                                if indent_comments {
                                    out.push_str(config.indent);
                                }
                            }
                            fmt(child, out)?;
                            out.push_str(line_ending);
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            indent_comments = true;
                            if !prev_is_comment {
                                out.push(' ');
                            }
                            fmt(child, out)?;
                            if !next_is_comment
                                && (loop_header_is_multiline || condition_is_multiline)
                            {
                                out.push_str(line_ending);
                            }
                        }
                        "variable" | "sequence" => {
                            if prev_is_comment
                                || (field_name == "variable"
                                    && (loop_header_is_multiline || condition_is_multiline))
                            {
                                let mut temp = String::new();
                                fmt(child, &mut temp)?;
                                out.push_str(&utils::indent_by(config.indent, &temp, line_ending));
                            } else if next_is_comment || condition_is_multiline {
                                let mut temp = String::new();
                                fmt(child, &mut temp)?;
                                out.push_str(&utils::indent_by_skip_first(
                                    config.indent,
                                    &temp,
                                    line_ending,
                                ));
                            } else {
                                fmt(child, out)?;
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            if !prev_is_comment
                                && (loop_header_is_multiline || condition_is_multiline)
                            {
                                out.push_str(line_ending);
                            }
                            fmt(child, out)?;
                        }
                        "body" => {
                            if !prev_is_comment {
                                out.push(' ');
                            }
                            if child.kind() == "braced_expression" {
                                fmt_multiline(child, true, out)?;
                            } else {
                                fmt_wrap_with_braces(child, out)?;
                            }
                        }
                        _ => unreachable!(),
                    },
                };
                Ok::<_, FormatError>(())
            })?;
        }
        "function_definition" => {
            handles_comments = true;

            let is_multiline = !same_line(node, node);

            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(line_ending);
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "name" => {
                            fmt(child, out)?;
                        }
                        "parameters" => {
                            if prev_is_comment {
                                out.push_str(line_ending);
                            }
                            fmt(child, out)?;
                        }
                        "body" => {
                            out.push_str(if prev_is_comment { line_ending } else { " " });
                            if is_multiline && child.kind() != "braced_expression" {
                                fmt_wrap_with_braces(child, out)?;
                            } else {
                                fmt_multiline(child, is_multiline, out)?;
                            }
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
        }
        "if_statement" => {
            handles_comments = true;

            let is_multiline = state.make_multiline || !same_line(node, node);

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
            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();

                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "if" => fmt(child, out)?,
                        "else" => {
                            if prev_is_comment {
                                out.push_str(line_ending);
                            } else {
                                out.push(' ');
                            }
                            fmt(child, out)?;
                        }
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(line_ending);
                                if indent_comments {
                                    out.push_str(config.indent);
                                }
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            indent_comments = true;
                            if prev_is_comment {
                                out.push_str(line_ending);
                            } else {
                                out.push(' ');
                            }
                            fmt(child, out)?;
                        }
                        "condition" => {
                            if !hug {
                                out.push_str(line_ending);

                                let mut temp = String::new();
                                fmt(child, &mut temp)?;
                                out.push_str(&utils::indent_by(config.indent, &temp, line_ending));
                            } else {
                                fmt(child, out)?;
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            if !hug {
                                out.push_str(line_ending);
                            }
                            fmt(child, out)?;
                        }
                        "consequence" | "alternative" => {
                            if prev_is_comment {
                                out.push_str(line_ending);
                            } else {
                                out.push(' ');
                            }
                            if is_multiline
                                && child.kind() != "braced_expression"
                                && (field_name != "alternative" || child.kind() != "if_statement")
                            {
                                fmt_wrap_with_braces(child, out)?;
                            } else {
                                fmt_multiline(child, is_multiline, out)?;
                            }
                        }
                        _ => unreachable!(),
                    },
                };
                Ok::<_, FormatError>(())
            })?;
        }
        "integer" => fmt_raw(node, out)?,
        "na" => fmt_raw(node, out)?,
        "parenthesized_expression" => {
            handles_comments = true;

            let is_multiline = !same_line(node, node);
            let is_empty = node.child_count() == 2;

            tree::for_each_child(&mut node.walk(), |i, child, field_name| {
                let maybe_prev = child.prev_sibling();

                match field_name {
                    None => match child.kind() {
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(line_ending);
                                out.push_str(config.indent);
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            fmt(child, out)?;
                        }
                        "body" => {
                            out.push_str(if is_multiline {
                                line_ending
                            } else if i == 1 {
                                ""
                            } else {
                                "; "
                            });

                            if is_multiline {
                                let mut temp = String::new();
                                fmt(child, &mut temp)?;
                                out.push_str(&utils::indent_by(config.indent, &temp, line_ending));
                            } else {
                                fmt(child, out)?;
                            }
                        }
                        "close" => {
                            if !is_empty {
                                out.push_str(if is_multiline { line_ending } else { "" });
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
        }
        "program" => {
            handles_comments = true;

            tree::for_each_child(&mut node.walk(), |_, child, _| {
                let maybe_prev = child.prev_sibling();

                match child.kind() {
                    "comment" if maybe_prev.is_some_and(|prev| same_line(prev, child)) => {
                        out.push(' ');
                    }
                    _ => {
                        out.push_str(&collapse_newlines(child, maybe_prev));
                    }
                }
                fmt(child, out)?;

                Ok::<_, FormatError>(())
            })?;
            out.push_str(line_ending);
        }
        "repeat_statement" => {
            handles_comments = true;

            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "repeat" => fmt(child, out)?,
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(line_ending);
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => {
                        if prev_is_comment {
                            out.push_str(line_ending);
                        } else {
                            out.push(' ');
                        }
                        match field_name {
                            "body" => {
                                if child.kind() == "braced_expression" {
                                    fmt_multiline(child, true, out)?;
                                } else {
                                    fmt_wrap_with_braces(child, out)?;
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                };
                Ok::<_, FormatError>(())
            })?;
        }
        "string" => {
            let maybe_string_content = field_optional(node, "content");
            match maybe_string_content {
                Some(string_content) => {
                    let mut content = String::new();
                    fmt(string_content, &mut content)?;
                    {
                        let mut formatted = String::with_capacity(content.len() + 2);
                        formatted.push('"');
                        let mut last_was_escape = false;
                        for char in content.chars() {
                            match char {
                                '"' if !last_was_escape => formatted.push_str("\\\""),
                                _ => formatted.push(char),
                            }
                            last_was_escape = char == '\\' && !last_was_escape;
                        }
                        formatted.push('"');
                        out.push_str(&formatted);
                    }
                }
                None => out.push_str("\"\""),
            }
        }
        "string_content" => fmt_with_indent_prefix(node, out)?,
        "unary_operator" => {
            handles_comments = true;
            push_all_comments(node, out)?;
            let operator = field(node, "operator")?;
            fmt(operator, out)?;
            out.push_str(if operator.kind() == "~" { " " } else { "" });
            fmt(field(node, "rhs")?, out)?;
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
            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();
                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "while" => fmt(child, out)?,
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(line_ending);
                                if indent_comments {
                                    out.push_str(config.indent);
                                }
                            }
                            fmt(child, out)?;
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            indent_comments = true;
                            out.push_str(if prev_is_comment { line_ending } else { " " });
                            fmt(child, out)?;
                        }
                        "condition" => {
                            if !hug {
                                out.push_str(line_ending);

                                let mut temp = String::new();
                                fmt(child, &mut temp)?;
                                out.push_str(&utils::indent_by(config.indent, &temp, line_ending));
                            } else {
                                fmt(child, out)?;
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            if !hug {
                                out.push_str(line_ending);
                            }
                            fmt(child, out)?;
                        }
                        "body" => {
                            out.push_str(if prev_is_comment { line_ending } else { " " });
                            if child.kind() == "braced_expression" {
                                fmt_multiline(child, true, out)?;
                            } else {
                                fmt_wrap_with_braces(child, out)?;
                            }
                        }
                        _ => unreachable!(),
                    },
                };
                Ok::<_, FormatError>(())
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
        let comments = node
            .children(&mut node.walk())
            .filter(|node| node.kind() == "comment")
            .collect::<Vec<_>>();

        if let Some(&comment) = comments.first()
            && config.stop_on_unhandled_comment
        {
            let start = comment.start_position();
            return Err(FormatError::UnhandledComment {
                comment: get_raw(comment),
                line: start.row,
                col: start.column,
            });
        }

        push_all_comments(node, out)?;
    }
    Ok(())
}
