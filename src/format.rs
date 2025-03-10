use {
    crate::{
        cli, config, tree,
        utils::{self},
    },
    console::style,
    ignore::Walk,
    itertools::Itertools,
    ropey::Rope,
    std::{path::PathBuf, time::Instant},
    thiserror::Error,
    tree_sitter::Node,
};

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub spaces: usize,
    pub stop_on_unhandled_comment: bool,
    pub line_ending: LineEnding,
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

    let file_config_pairs = files
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
                .filter_map(|entry_result| match entry_result {
                    Ok(entry) => {
                        let path = entry.into_path();
                        if path
                            .extension()
                            .map(|ext| ext == "R" || ext == "r")
                            .unwrap_or(false)
                        {
                            Some(Ok(path))
                        } else {
                            None
                        }
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
    for (paths, config) in file_config_pairs.into_iter() {
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
            let new = match format(tree.root_node(), &rope, Config {
                spaces: config.spaces,
                stop_on_unhandled_comment,
                line_ending: LineEnding::Auto,
            }) {
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
                    print_diff(&old, &new);
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

// from: https://github.com/mitsuhiko/similar/blob/main/examples/terminal-inline.rs
pub fn print_diff(old: &str, new: &str) {
    use {
        console::Style,
        similar::{ChangeTag, TextDiff},
    };

    struct Line(Option<usize>);

    impl std::fmt::Display for Line {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            match self.0 {
                None => write!(f, "    "),
                Some(idx) => write!(f, "{:<4}", idx + 1),
            }
        }
    }

    let diff = TextDiff::from_lines(old, new);

    for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
        if idx > 0 {
            eprintln!("{:-^1$}", "-", 80);
        }
        for op in group {
            for change in diff.iter_inline_changes(op) {
                let (sign, style) = match change.tag() {
                    ChangeTag::Delete => ("-", Style::new().red()),
                    ChangeTag::Insert => ("+", Style::new().green()),
                    ChangeTag::Equal => (" ", Style::new().dim()),
                };
                eprint!(
                    "{}{} |{}",
                    console::style(Line(change.old_index())).dim(),
                    console::style(Line(change.new_index())).dim(),
                    style.apply_to(sign).bold(),
                );
                for (emphasized, value) in change.iter_strings_lossy() {
                    if emphasized {
                        eprint!("{}", style.apply_to(value).underlined().on_black());
                    } else {
                        eprint!("{}", style.apply_to(value));
                    }
                }
                if change.missing_newline() {
                    eprintln!();
                }
            }
        }
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

    let formatted = utils::remove_indent_prefix(&traverse(
        node,
        rope,
        line_ending,
        config,
        State::default(),
    )?);

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
) -> Result<String, FormatError> {
    // enum Fmt {
    //     Normal,
    //     Raw,
    //     WrapWithBraces,
    //     WithIndentPrefix,
    //     MakeMultiline,
    // }

    let kind = node.kind();
    let fmt = |node: Node| traverse(node, rope, line_ending, config, State::default());
    let fmt_raw = |node: Node| rope.byte_slice(node.byte_range()).to_string();
    let fmt_multiline = |node: Node, make_multiline: bool| {
        traverse(
            node,
            rope,
            line_ending,
            config,
            State::make_multiline(make_multiline),
        )
    };
    let fmt_with_indent_prefix = |node: Node| utils::add_indent_prefix(&fmt_raw(node));
    let fmt_wrap_with_braces = |node: Node| -> Result<String, FormatError> {
        Ok(format!(
            "{{{}}}",
            utils::indent_by_with_newlines(config.spaces, fmt(node)?, line_ending)
        ))
    };
    // let fmt_with = |node: Node, action: Fmt| match action {
    //     Fmt::Normal => fmt(node),
    //     Fmt::Raw => Ok(fmt_raw(node)),
    //     Fmt::WrapWithBraces => fmt_wrap_with_braces(node),
    //     Fmt::WithIndentPrefix => Ok(fmt_with_indent_prefix(node)),
    //     Fmt::MakeMultiline => fmt_multiline(node, state.make_multiline),
    // };

    fn field<'a>(node: Node<'a>, field_name: &'static str) -> Result<Node<'a>, FormatError> {
        let kind = node.kind();
        node.child_by_field_name(field_name)
            .ok_or(FormatError::MissingField {
                kind,
                field: field_name,
            })
    }
    fn field_optional<'a>(node: Node<'a>, field_name: &'static str) -> Option<Node<'a>> {
        node.child_by_field_name(field_name)
    }

    let collapse_newlines = |node: Node, maybe_prev: Option<Node>| {
        line_ending.repeat(
            maybe_prev
                .map(|prev| usize::clamp(node.start_position().row - prev.end_position().row, 1, 2))
                .unwrap_or(0),
        )
    };
    let is_comment = |maybe_node: Option<Node>| {
        maybe_node
            .map(|next| next.kind() == "comment")
            .unwrap_or(false)
    };

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
    let is_fmt_skip_comment = |node: Node| {
        node.kind() == "comment"
            && rope
                .byte_slice(node.byte_range())
                .to_string()
                .contains("fmt: skip")
    };
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
    let check = |node: Node| -> Result<(), FormatError> {
        if node.is_missing() {
            Err(missing(node))
        } else if node.is_error() {
            Err(syntax_error(node))
        } else {
            Ok(())
        }
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
        let prev_is_fmt_skip = node
            .prev_sibling()
            .map(|prev| {
                is_fmt_skip_comment(prev)
                    && prev
                        .prev_sibling()
                        .map(|before_prev| !same_line(before_prev, prev))
                        .unwrap_or(true)
            })
            .unwrap_or(false);
        let next_is_fmt_skip = node
            .next_sibling()
            .map(|next| is_fmt_skip_comment(next) && same_line(node, next))
            .unwrap_or(false);

        if prev_is_fmt_skip || next_is_fmt_skip {
            dbg!(fmt_raw(node));
            return Ok(fmt_with_indent_prefix(node));
        }
    }

    if node.kind() == "comment" {
        let raw = fmt_raw(node);
        let raw = raw.trim_end();

        let mut chars = raw.chars();

        let _ = chars.next();
        // reformat comments like #foo to # foo but keep #' foo
        return Ok(match chars.next() {
            Some('\'') => match chars.next() {
                Some(' ') => raw.into(),
                Some(other) => {
                    let rest = chars.collect::<String>();
                    // avoid formatting #'foo'
                    if rest.contains('\'') {
                        raw.into()
                    } else {
                        format!("#' {other}{}", rest)
                    }
                }
                None => "#'".into(),
            },
            Some('#' | '!' | ' ') => raw.into(),
            Some(other) => format!("# {other}{}", chars.collect::<String>()),
            None => "#".into(),
        });
    }

    if node.is_extra() {
        log::warn!("node of kind {} is extra but is not a comment", node.kind());
    }

    if !node.is_named() {
        return Ok(fmt_raw(node));
    }

    let mut handles_comments = false;

    let result = match kind {
        "argument" => {
            let maybe_name = field_optional(node, "name");
            let maybe_value = field_optional(node, "value");

            // support the switch fallthrough
            let mut cursor = node.walk();
            let has_equal = node.children(&mut cursor).any(|node| node.kind() == "=");

            match (maybe_name, maybe_value) {
                (Some(name), Some(value)) => {
                    format!("{} = {}", fmt(name)?, fmt(value)?)
                }
                (None, Some(value)) => fmt(value)?.to_string(),
                (Some(name), None) if has_equal => format!("{} = ", fmt(name)?),
                (Some(name), None) => fmt(name)?.to_string(),
                (None, None) => String::new(),
            }
        }
        "arguments" => {
            handles_comments = true;

            check(field(node, "open")?)?;
            check(field(node, "close")?)?;
            let is_multiline = !same_line(node, node);

            let mut maybe_prev = None;
            let mut is_first_arg = true;
            node.children(&mut node.walk())
                .skip(1)
                .take(node.child_count() - 2)
                .map(|child| {
                    let maybe_prev = {
                        let tmp = maybe_prev;
                        maybe_prev = Some(child);
                        tmp
                    };
                    let formatted = fmt(child)?;
                    if child.kind() == "comment" {
                        return Ok(match maybe_prev {
                            Some(prev) if same_line(prev, child) => {
                                format!(" {formatted}")
                            }
                            Some(prev) => format!(
                                "{}{formatted}",
                                line_ending.repeat(usize::clamp(
                                    child.start_position().row - end_position(prev).row,
                                    1,
                                    2,
                                ))
                            ),
                            None => formatted.to_string(),
                        });
                    }
                    if child.kind() == "comma" {
                        is_first_arg = false;
                        return Ok(
                            if maybe_prev
                                .map(|prev| prev.kind() == "comment")
                                .unwrap_or(false)
                            {
                                format!("{line_ending},")
                            } else {
                                ",".to_string()
                            },
                        );
                    }
                    let result = format!(
                        "{}{}",
                        if is_first_arg {
                            if maybe_prev
                                .map(|prev| prev.kind() == "comment")
                                .unwrap_or(false)
                            {
                                line_ending.into()
                            } else {
                                "".into()
                            }
                        } else if is_multiline {
                            line_ending.repeat(usize::clamp(
                                maybe_prev
                                    .map(|prev| child.start_position().row - end_position(prev).row)
                                    .unwrap_or(1),
                                1,
                                2,
                            ))
                        } else {
                            " ".into()
                        },
                        formatted
                    );
                    is_first_arg = false;
                    Ok(result)
                })
                .collect::<Result<String, FormatError>>()?
        }
        "binary_operator" => {
            handles_comments = true;

            let lhs = field(node, "lhs")?;
            let operator = field(node, "operator")?;
            let rhs = field(node, "rhs")?;

            let comments = node
                .children(&mut node.walk())
                .filter(|node| node.kind() == "comment")
                .collect::<Vec<Node>>();
            let indent = format!("{line_ending}{}", " ".repeat(config.spaces));
            let first_comment_sep = comments
                .first()
                .map(|&comment| {
                    if same_line(operator, comment) {
                        " "
                    } else {
                        indent.as_str()
                    }
                })
                .unwrap_or("");
            let comments_fmt = comments
                .into_iter()
                .map(fmt)
                .collect::<Result<Vec<String>, FormatError>>()?
                .join(&indent);

            let is_multiline = !same_line(lhs, rhs);
            let has_spacing = operator.kind() == ":";
            format!(
                "{}{}{}{}{}{}{}",
                fmt(lhs)?,
                if has_spacing { "" } else { " " },
                fmt(operator)?,
                first_comment_sep,
                comments_fmt,
                if is_multiline {
                    line_ending
                } else if has_spacing {
                    ""
                } else {
                    " "
                },
                if is_multiline {
                    utils::indent_by(config.spaces, fmt(rhs)?, line_ending)
                } else {
                    fmt(rhs)?
                }
            )
        }
        "braced_expression" => {
            handles_comments = true;

            let is_multiline = !same_line(node, node) || state.make_multiline;
            let is_empty = node.child_count() == 2;

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
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
                                out.push_str(&" ".repeat(config.spaces));
                            }
                            out.push_str(&fmt(child)?);
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            out.push_str(&fmt(child)?);
                        }
                        "body" => {
                            if i == 1 {
                                if !is_empty {
                                    out.push_str(if is_multiline { line_ending } else { " " });
                                }
                            } else {
                                out.push_str(&if is_multiline {
                                    collapse_newlines(child, maybe_prev)
                                } else {
                                    "; ".into()
                                })
                            }

                            out.push_str(&if is_multiline {
                                utils::indent_by(config.spaces, &fmt(child)?, line_ending)
                            } else {
                                fmt(child)?
                            });
                        }
                        "close" => {
                            if !is_empty {
                                out.push_str(if is_multiline { line_ending } else { " " });
                            }
                            out.push_str(&fmt(child)?);
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
            out
        }
        "call" => {
            let function = field(node, "function")?;
            let arguments = field(node, "arguments")?;
            let is_multiline = !same_line(arguments, arguments);

            let function_fmt = fmt(function)?;
            let arguments_fmt = fmt(arguments)?;
            // note: `extract_operator` has higher precedence than calls, so we must add special indentation
            // `namespace_operator` doesn't allow newlines, so there shouldn't be a problem
            let additional_indent = match function.kind() {
                "extract_operator" => {
                    let lhs = field(function, "lhs")?;
                    let maybe_rhs = field_optional(function, "rhs");
                    maybe_rhs.map(|rhs| !same_line(lhs, rhs)).unwrap_or(false)
                }
                _ => false,
            };
            format!(
                "{}{}",
                function_fmt,
                if is_multiline
                    // don't wrap calls like foo({ bar })
                    && !(arguments.named_child_count() == 1 && {
                        let argument = arguments.named_child(0).unwrap();
                        argument.kind() == "argument"
                            && argument.child_count() == 1
                            && argument.child(0).unwrap().kind() == "braced_expression"
                    })
                {
                    let out = format!(
                        "({})",
                        utils::indent_by_with_newlines(config.spaces, arguments_fmt, line_ending)
                    );
                    // we need additional indent if we are lhs of multiline extract operator
                    if additional_indent {
                        utils::indent_by_skip_first(config.spaces, out, line_ending)
                    } else {
                        out
                    }
                } else {
                    format!("({arguments_fmt})")
                }
            )
        }
        "complex" => fmt_raw(node),
        "extract_operator" => {
            handles_comments = true;

            let lhs = field(node, "lhs")?;
            let operator = field(node, "operator")?;
            let maybe_rhs = field_optional(node, "rhs");

            let comments = node
                .children(&mut node.walk())
                .filter(|node| node.kind() == "comment")
                .collect::<Vec<Node>>();
            let indent = format!("{line_ending}{}", " ".repeat(config.spaces));
            let first_comment_sep = comments
                .first()
                .map(|&comment| {
                    if same_line(operator, comment) {
                        " "
                    } else {
                        indent.as_str()
                    }
                })
                .unwrap_or("");
            let comments_fmt = comments
                .into_iter()
                .map(fmt)
                .collect::<Result<Vec<String>, FormatError>>()?
                .join(&indent);

            let (is_multiline, rhs_fmt) = maybe_rhs
                .map(|rhs| (!same_line(lhs, rhs), fmt(rhs)))
                .unwrap_or_else(|| (false, Ok("".into())));
            format!(
                "{}{}{}{}{}{}",
                fmt(lhs)?,
                fmt(operator)?,
                first_comment_sep,
                comments_fmt,
                if is_multiline { line_ending } else { "" },
                if is_multiline {
                    utils::indent_by(config.spaces, rhs_fmt?, line_ending)
                } else {
                    rhs_fmt?
                }
            )
        }
        "float" => fmt_raw(node),
        "for_statement" => {
            handles_comments = true;

            let condition_is_multiline = !same_line(field(node, "open")?, field(node, "close")?);
            let loop_header_is_multiline =
                !same_line(field(node, "variable")?, field(node, "sequence")?);

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
            let mut indent_comments = false;
            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();

                let prev_is_comment = maybe_prev
                    .map(|node| node.kind() == "comment")
                    .unwrap_or(false);
                let next_is_comment = child
                    .next_sibling()
                    .map(|next| next.kind() == "comment")
                    .unwrap_or(false);

                match field_name {
                    None => match child.kind() {
                        "for" => out.push_str("for"),
                        "in" => {
                            if prev_is_comment {
                                out.push_str(&" ".repeat(config.spaces));
                            } else if loop_header_is_multiline {
                                out.push_str(line_ending);
                                out.push_str(&" ".repeat(config.spaces));
                            } else {
                                out.push(' ');
                            }
                            out.push_str("in");
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
                                    out.push_str(&" ".repeat(config.spaces));
                                }
                            }
                            out.push_str(&fmt(child)?);
                            out.push_str(line_ending)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            indent_comments = true;
                            if !prev_is_comment {
                                out.push(' ');
                            }
                            out.push('(');
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
                                out.push_str(&utils::indent_by(
                                    config.spaces,
                                    &fmt(child)?,
                                    line_ending,
                                ));
                            } else if next_is_comment || condition_is_multiline {
                                out.push_str(&utils::indent_by_skip_first(
                                    config.spaces,
                                    &fmt(child)?,
                                    line_ending,
                                ));
                            } else {
                                out.push_str(&fmt(child)?);
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            if !prev_is_comment
                                && (loop_header_is_multiline || condition_is_multiline)
                            {
                                out.push_str(line_ending);
                            }
                            out.push(')')
                        }
                        "body" => {
                            if !prev_is_comment {
                                out.push(' ');
                            }
                            out.push_str(&if child.kind() == "braced_expression" {
                                fmt_multiline(child, true)?
                            } else {
                                fmt_wrap_with_braces(child)?
                            })
                        }
                        _ => unreachable!(),
                    },
                };
                Ok::<_, FormatError>(())
            })?;
            out
        }
        "function_definition" => {
            let name = field(node, "name")?;
            let parameters = field(node, "parameters")?;
            let body = field(node, "body")?;
            let is_multiline = !same_line(node, node);

            let parameters_fmt = fmt(parameters)?;
            format!(
                "{}({}) {}",
                fmt(name)?,
                if parameters_fmt.is_empty() || same_line(parameters, parameters) {
                    parameters_fmt
                } else {
                    utils::indent_by_with_newlines(config.spaces, parameters_fmt, line_ending)
                },
                if is_multiline && body.kind() != "braced_expression" {
                    fmt_wrap_with_braces(body)?
                } else {
                    fmt_multiline(body, is_multiline)?
                },
            )
        }
        "if_statement" => {
            handles_comments = true;

            let is_multiline = state.make_multiline || !same_line(node, node);
            let condition_is_multiline = !same_line(field(node, "open")?, field(node, "close")?);

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
            let mut indent_comments = false;
            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();

                let prev_is_comment = maybe_prev
                    .map(|node| node.kind() == "comment")
                    .unwrap_or(false);

                match field_name {
                    None => match child.kind() {
                        "if" => out.push_str("if"),
                        "else" => {
                            if !prev_is_comment {
                                out.push(' ');
                            }
                            out.push_str("else")
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
                                    out.push_str(&" ".repeat(config.spaces));
                                }
                            }
                            out.push_str(&fmt(child)?);
                            out.push_str(line_ending)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            indent_comments = true;
                            if !prev_is_comment {
                                out.push(' ');
                            }
                            out.push('(');
                        }
                        "condition" => {
                            let next_is_comment = child
                                .next_sibling()
                                .map(|next| next.kind() == "comment")
                                .unwrap_or(false);
                            if condition_is_multiline
                                && !(child.kind() == "braced_expression"
                                    && !(prev_is_comment || next_is_comment))
                            {
                                if !prev_is_comment {
                                    out.push_str(line_ending);
                                }
                                out.push_str(&utils::indent_by(
                                    config.spaces,
                                    fmt(child)?,
                                    line_ending,
                                ));
                                if !next_is_comment {
                                    out.push_str(line_ending);
                                }
                            } else {
                                out.push_str(&fmt(child)?);
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            out.push(')')
                        }
                        "consequence" | "alternative" => {
                            if !prev_is_comment {
                                out.push(' ');
                            }
                            out.push_str(&if is_multiline
                                && child.kind() != "braced_expression"
                                && (field_name != "alternative" || child.kind() != "if_statement")
                            {
                                fmt_wrap_with_braces(child)?
                            } else {
                                fmt_multiline(child, is_multiline)?
                            })
                        }
                        _ => unreachable!(),
                    },
                };
                Ok::<_, FormatError>(())
            })?;
            out
        }
        "integer" => fmt_raw(node),
        "na" => fmt_raw(node),
        "namespace_operator" => {
            let lhs = field(node, "lhs")?;
            let op = field(node, "operator")?;
            let maybe_rhs = field_optional(node, "rhs");
            format!("{}{}{}", fmt(lhs)?, op.kind(), match maybe_rhs {
                Some(rhs) => fmt(rhs)?,
                None => "".into(),
            })
        }
        "parameter" => {
            let name = field(node, "name")?;
            let maybe_default = field_optional(node, "default");

            let name = fmt(name)?;
            match maybe_default {
                Some(default) => format!("{name} = {}", fmt(default)?),
                None => name,
            }
        }
        "parameters" => {
            handles_comments = true;

            let is_multiline = !same_line(node, node);

            let mut maybe_prev = None;
            let mut is_first_param = true;
            let mut cursor = node.walk();
            node.children(&mut cursor)
                .skip(1)
                .take(node.child_count() - 2)
                .map(|child| {
                    let maybe_prev = {
                        let tmp = maybe_prev;
                        maybe_prev = Some(child);
                        tmp
                    };
                    let tmp = fmt(child)?;
                    if child.kind() == "comment" {
                        return Ok(match maybe_prev {
                            Some(prev) if same_line(prev, child) => {
                                format!(" {tmp}")
                            }
                            Some(_) => format!("{line_ending}{tmp}"),
                            None => tmp.to_string(),
                        });
                    }
                    if child.kind() == "comma" {
                        is_first_param = false;
                        return Ok(
                            if maybe_prev
                                .map(|node| node.kind() == "comment")
                                .unwrap_or(false)
                            {
                                format!("{line_ending},")
                            } else {
                                ",".to_string()
                            },
                        );
                    }
                    let result = format!(
                        "{}{}",
                        if is_first_param {
                            if maybe_prev
                                .map(|node| node.kind() == "comment")
                                .unwrap_or(false)
                            {
                                line_ending
                            } else {
                                ""
                            }
                        } else if is_multiline {
                            line_ending
                        } else {
                            " "
                        },
                        tmp
                    );
                    is_first_param = false;
                    Ok(result)
                })
                .collect::<Result<String, FormatError>>()?
        }
        "parenthesized_expression" => {
            handles_comments = true;

            let is_multiline = !same_line(node, node);
            let is_empty = node.child_count() == 2;

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
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
                                out.push_str(&" ".repeat(config.spaces));
                            }
                            out.push_str(&fmt(child)?);
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            out.push_str(&fmt(child)?);
                        }
                        "body" => {
                            if i == 1 {
                                out.push_str(if is_multiline { line_ending } else { "" });
                            } else {
                                out.push_str(&if is_multiline {
                                    collapse_newlines(child, maybe_prev)
                                } else {
                                    "; ".into()
                                })
                            }

                            out.push_str(&if is_multiline {
                                utils::indent_by(config.spaces, &fmt(child)?, line_ending)
                            } else {
                                fmt(child)?
                            });
                        }
                        "close" => {
                            if !is_empty {
                                out.push_str(if is_multiline { line_ending } else { "" });
                            }
                            out.push_str(&fmt(child)?);
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
            out
        }
        "program" => {
            handles_comments = true;

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
            tree::for_each_child(&mut node.walk(), |_, child, _| {
                let maybe_prev = child.prev_sibling();

                match child.kind() {
                    "comment"
                        if maybe_prev
                            .map(|prev| same_line(prev, child))
                            .unwrap_or(false) =>
                    {
                        out.push(' ');
                    }
                    _ => {
                        out.push_str(&collapse_newlines(child, maybe_prev));
                    }
                }
                out.push_str(&fmt(child)?);

                Ok::<_, FormatError>(())
            })?;
            out.push_str(line_ending);
            out
        }
        "repeat_statement" => {
            handles_comments = true;

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();

                let prev_is_comment = maybe_prev
                    .map(|node| node.kind() == "comment")
                    .unwrap_or(false);

                match field_name {
                    None => match child.kind() {
                        "repeat" => out.push_str("repeat"),
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else if !prev_is_comment {
                                out.push_str(line_ending);
                            }
                            out.push_str(&fmt(child)?);
                            out.push_str(line_ending)
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => {
                        if !prev_is_comment {
                            out.push(' ');
                        }
                        match field_name {
                            "body" => out.push_str(&if child.kind() == "braced_expression" {
                                fmt_multiline(child, true)?
                            } else {
                                fmt_wrap_with_braces(child)?
                            }),
                            _ => unreachable!(),
                        }
                    }
                };
                Ok::<_, FormatError>(())
            })?;
            out
        }
        "string" => {
            let maybe_string_content = field_optional(node, "content");
            match maybe_string_content {
                Some(string_content) => {
                    let content = fmt(string_content)?;
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
                        formatted
                    }
                }
                None => "\"\"".to_string(),
            }
        }
        "string_content" => fmt_with_indent_prefix(node),
        "subset" => {
            let function = field(node, "function")?;
            let arguments = field(node, "arguments")?;

            let function_fmt = fmt(function)?;
            let arguments_fmt = fmt(arguments)?;
            format!(
                "{function_fmt}[{}]",
                if same_line(arguments, arguments) {
                    arguments_fmt
                } else {
                    utils::indent_by_with_newlines(config.spaces, arguments_fmt, line_ending)
                }
            )
        }
        "subset2" => {
            let function = field(node, "function")?;
            let arguments = field(node, "arguments")?;

            let function_fmt = fmt(function)?;
            let arguments_fmt = fmt(arguments)?;
            format!(
                "{function_fmt}[[{}]]",
                if same_line(arguments, arguments) {
                    arguments_fmt
                } else {
                    utils::indent_by_with_newlines(config.spaces, arguments_fmt, line_ending)
                }
            )
        }
        "unary_operator" => {
            let operator = field(node, "operator")?;
            let spacing = if operator.kind() == "~" { " " } else { "" };
            format!("{}{spacing}{}", fmt(operator)?, fmt(field(node, "rhs")?)?)
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

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
            let mut indent_comments = false;
            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();

                let prev_is_comment = maybe_prev
                    .map(|node| node.kind() == "comment")
                    .unwrap_or(false);

                match field_name {
                    None => match child.kind() {
                        "while" => out.push_str("while"),
                        "comment" => {
                            if let Some(prev) = maybe_prev
                                && same_line(prev, child)
                            {
                                out.push(' ');
                            } else {
                                out.push_str(&collapse_newlines(child, maybe_prev));
                                if indent_comments {
                                    out.push_str(&" ".repeat(config.spaces));
                                }
                            }
                            out.push_str(&fmt(child)?);
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "open" => {
                            indent_comments = true;
                            out.push_str(if prev_is_comment { line_ending } else { " " });
                            out.push_str(&fmt(child)?);
                        }
                        "condition" => {
                            if !hug {
                                out.push_str(line_ending);
                                out.push_str(&utils::indent_by(
                                    config.spaces,
                                    fmt(child)?,
                                    line_ending,
                                ));
                            } else {
                                out.push_str(&fmt(child)?);
                            }
                        }
                        "close" => {
                            indent_comments = false;
                            if !hug {
                                out.push_str(line_ending);
                            }
                            out.push_str(&fmt(child)?);
                        }
                        "body" => {
                            out.push_str(if prev_is_comment { line_ending } else { " " });
                            out.push_str(&if child.kind() == "braced_expression" {
                                fmt_multiline(child, true)?
                            } else {
                                fmt_wrap_with_braces(child)?
                            })
                        }
                        _ => unreachable!(),
                    },
                };
                Ok::<_, FormatError>(())
            })?;
            out
        }
        // SIMPLE
        "break" => "break".into(),
        "comma" => ",".into(),
        "comment" => fmt_raw(node),
        "dot_dot_i" => fmt_raw(node),
        "dots" => "...".into(),
        "escape_sequence" => fmt_raw(node),
        "false" => "FALSE".into(),
        "identifier" => fmt_raw(node),
        "inf" => "Inf".into(),
        "nan" => "NaN".into(),
        "next" => "next".into(),
        "null" => "NULL".into(),
        "return" => "return".into(),
        "true" => "TRUE".into(),
        unknown => {
            log::error!("unknown node kind: {unknown}");
            return Err(FormatError::Unknown {
                kind,
                raw: fmt_raw(node),
            });
        }
    };

    Ok(if handles_comments {
        result
    } else {
        let comments = node
            .children(&mut node.walk())
            .filter(|node| node.kind() == "comment")
            .collect::<Vec<_>>();
        if let Some(&comment) = comments.first()
            && config.stop_on_unhandled_comment
        {
            let start = comment.start_position();
            return Err(FormatError::UnhandledComment {
                comment: fmt(comment)?,
                line: start.row,
                col: start.column,
            });
        }

        comments
            .into_iter()
            .map(fmt)
            .chain(std::iter::once(Ok(result)))
            .collect::<Result<Vec<String>, FormatError>>()?
            .join(line_ending)
    })
}
