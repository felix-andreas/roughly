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
    for (paths, config) in file_config_pairs {
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
            "{{{line_ending}{}{line_ending}}}",
            utils::indent_by(config.spaces, fmt(node)?, line_ending)
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
                        format!("#' {other}{rest}")
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
                        "comma" => {
                            if is_multiline
                                && (maybe_prev
                                    .is_none_or(|node| ["comment", "comma"].contains(&node.kind()))
                                    || i == 1)
                            {
                                out.push_str(line_ending);
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

                            out.push_str(&if is_multiline && !hug {
                                utils::indent_by(config.spaces, &fmt(child)?, line_ending)
                            } else {
                                fmt(child)?
                            });
                        }
                        "close" => {
                            if is_multiline && !hug && !is_empty {
                                out.push_str(line_ending);
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
        "binary_operator" => {
            handles_comments = true;

            let is_multiline = !same_line(field(node, "lhs")?, field(node, "rhs")?);
            let has_spacing = field(node, "operator")?.kind() == ":";

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
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
                                out.push_str(&collapse_newlines(child, maybe_prev));
                                out.push_str(&" ".repeat(config.spaces));
                            }
                            out.push_str(&fmt(child)?);
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "lhs" => {
                            out.push_str(&fmt(child)?);
                        }
                        "operator" => {
                            if !has_spacing {
                                out.push(' ');
                            }
                            out.push_str(&fmt(child)?);
                        }
                        "rhs" => {
                            if is_multiline {
                                out.push_str(line_ending);
                                out.push_str(&utils::indent_by(
                                    config.spaces,
                                    &fmt(child)?,
                                    line_ending,
                                ));
                            } else {
                                if !has_spacing {
                                    out.push(' ');
                                }
                                out.push_str(&fmt(child)?);
                            }
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
            out
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
                                });
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

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
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
                                out.push_str(&collapse_newlines(child, maybe_prev));
                                out.push_str(&" ".repeat(config.spaces));
                            }
                            out.push_str(&fmt(child)?);
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "function" => {
                            out.push_str(&fmt(child)?);
                        }
                        "arguments" => {
                            if additional_indent {
                                out.push_str(&utils::indent_by_skip_first(
                                    config.spaces,
                                    &fmt(child)?,
                                    line_ending,
                                ));
                            } else {
                                out.push_str(&fmt(child)?);
                            }
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
            out
        }
        "complex" => fmt_raw(node),
        "extract_operator" | "namespace_operator" => {
            handles_comments = true;

            let lhs = field(node, "lhs")?;
            let is_multiline = field_optional(node, "rhs").is_some_and(|rhs| !same_line(lhs, rhs));

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
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
                                out.push_str(&collapse_newlines(child, maybe_prev));
                                out.push_str(&" ".repeat(config.spaces));
                            }
                            out.push_str(&fmt(child)?);
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "lhs" => {
                            out.push_str(&fmt(child)?);
                        }
                        "operator" => {
                            out.push_str(&fmt(child)?);
                        }
                        "rhs" => {
                            if is_multiline {
                                out.push_str(line_ending);
                                out.push_str(&utils::indent_by(
                                    config.spaces,
                                    &fmt(child)?,
                                    line_ending,
                                ));
                            } else {
                                out.push_str(&fmt(child)?);
                            }
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
            out
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

                let prev_is_comment = is_comment(maybe_prev);
                let next_is_comment = is_comment(child.next_sibling());

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
                            out.push(')');
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
            handles_comments = true;

            let is_multiline = !same_line(node, node);

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
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
                                out.push_str(&collapse_newlines(child, maybe_prev));
                            }
                            out.push_str(&fmt(child)?);
                        }
                        _ => unreachable!(),
                    },
                    Some(field_name) => match field_name {
                        "name" => {
                            out.push_str(&fmt(child)?);
                        }
                        "parameters" => {
                            if prev_is_comment {
                                out.push_str(line_ending);
                            }
                            out.push_str(&fmt(child)?);
                        }
                        "body" => {
                            out.push_str(if prev_is_comment { line_ending } else { " " });
                            out.push_str(&if is_multiline && child.kind() != "braced_expression" {
                                fmt_wrap_with_braces(child)?
                            } else {
                                fmt_multiline(child, is_multiline)?
                            });
                        }
                        _ => unreachable!(),
                    },
                }
                Ok::<_, FormatError>(())
            })?;
            out
        }
        "if_statement" => {
            handles_comments = true;

            let is_multiline = state.make_multiline || !same_line(node, node);
            let condition_is_multiline = !same_line(field(node, "open")?, field(node, "close")?);

            let mut out = String::with_capacity(node.end_byte() - node.start_byte());
            let mut indent_comments = false;
            tree::for_each_child(&mut node.walk(), |_, child, field_name| {
                let maybe_prev = child.prev_sibling();

                let prev_is_comment = is_comment(maybe_prev);

                match field_name {
                    None => match child.kind() {
                        "if" => out.push_str("if"),
                        "else" => {
                            if !prev_is_comment {
                                out.push(' ');
                            }
                            out.push_str("else");
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
                            out.push('(');
                        }
                        "condition" => {
                            let next_is_comment = child
                                .next_sibling()
                                .is_some_and(|next| next.kind() == "comment");
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
                            out.push(')');
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
                            });
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
        "parameter" => {
            let name = field(node, "name")?;
            let maybe_default = field_optional(node, "default");

            let name = fmt(name)?;
            match maybe_default {
                Some(default) => format!("{name} = {}", fmt(default)?),
                None => name,
            }
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
                                });
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
                    "comment" if maybe_prev.is_some_and(|prev| same_line(prev, child)) => {
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
                let prev_is_comment = is_comment(maybe_prev);

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
                            out.push_str(line_ending);
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
                let prev_is_comment = is_comment(maybe_prev);

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
                            });
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
