use {
    ropey::Rope,
    tower_lsp::lsp_types::{Position, Range},
};

pub fn position_to_index(position: Position, rope: &Rope) -> Result<usize, ropey::Error> {
    let line = position.line as usize;
    let line = rope.try_line_to_char(line)?;
    Ok(line + position.character as usize)
}

pub fn index_to_position(index: usize, rope: &Rope) -> Result<Position, ropey::Error> {
    let line = rope.try_char_to_line(index)?;
    let char = index - rope.line_to_char(line);
    Ok(Position {
        line: line as u32,
        character: char as u32,
    })
}

pub fn lsp_range_to_rope_range(
    range: Range,
    rope: &Rope,
) -> Result<std::ops::Range<usize>, ropey::Error> {
    let start = position_to_index(range.start, rope)?;
    let end = position_to_index(range.end, rope)?;
    Ok(start..end)
}

pub fn rope_range_to_lsp_range(
    range: std::ops::Range<usize>,
    rope: &Rope,
) -> Result<Range, ropey::Error> {
    let start = index_to_position(range.start, rope)?;
    let end = index_to_position(range.end, rope)?;
    Ok(Range { start, end })
}

// adapted from https://docs.rs/indent/latest/src/indent/lib.rs.html#27-32
pub fn indent_by(prefix: &str, input: &str, line_ending: &str) -> String {
    indent(prefix, input, line_ending, true)
}

pub fn indent_by_skip_first(prefix: &str, input: &str, line_ending: &str) -> String {
    indent(prefix, input, line_ending, false)
}

#[inline]
fn indent(prefix: &str, input: &str, line_ending: &str, indent_all: bool) -> String {
    let length = input.len();
    let mut output = String::with_capacity(length + length / 2);

    for (i, line) in input.lines().enumerate() {
        if i > 0 {
            output.push_str(line_ending);
            if !line.is_empty() {
                output.push_str(prefix);
            }
        } else if indent_all && !line.is_empty() {
            output.push_str(prefix);
        }

        output.push_str(line);
    }

    // checking for \n works for \n and \r\n (in case file doesn't have target line ending yet)
    if input.ends_with('\n') {
        output.push_str(line_ending);
    }

    output
}

pub fn add_indent_prefix(input: &str) -> String {
    let mut output = String::with_capacity(input.len() + 5);
    for char in input.chars() {
        output.push(char);
        if char == '\n' {
            output.push('\x02')
        }
    }
    output
}

pub fn remove_indent_prefix(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut tmp = String::with_capacity(256);
    let mut push_to_temp = true;
    for char in input.chars() {
        if char == '\x02' {
            push_to_temp = false;
            continue;
        }

        if push_to_temp {
            tmp.push(char);
        } else {
            output.push(char);
        }

        if char == '\n' {
            if push_to_temp {
                output.push_str(&tmp);
            }
            tmp.clear();
            push_to_temp = true;
        }
    }
    output.push_str(&tmp);

    output
}

// adapted from https://doc.rust-lang.org/stable/nightly-rustc/src/clippy_utils/str_utils.rs.html

/// ```
/// use roughly::utils::to_camel_case;
/// assert_eq!(to_camel_case("foo_bar"), "fooBar");
/// assert_eq!(to_camel_case("fooXY"), "fooXY");
/// assert_eq!(to_camel_case("foo_x_y"), "fooXY");
/// assert_eq!(to_camel_case("foo_X_Y"), "fooXY");
/// assert_eq!(to_camel_case("foo_x_Y"), "fooXY");
/// ```
pub fn to_camel_case(name: &str) -> String {
    let mut camel = String::new();
    let mut chars = name.chars();
    chars.next().inspect(|&c| camel.push(c));

    let mut up = false;
    for char in chars {
        if char == '_' {
            up = true;
            continue;
        }
        if up {
            up = false;
            camel.extend(char.to_uppercase());
        } else {
            camel.push(char);
        }
    }
    camel
}

/// ```
/// use roughly::utils::to_snake_case;
/// assert_eq!(to_snake_case("fooBar"), "foo_bar");
/// assert_eq!(to_snake_case("fooXY"), "foo_x_y");
/// assert_eq!(to_snake_case("foo_bar"), "foo_bar");
/// assert_eq!(to_snake_case("Foo_Bar"), "foo_bar");
/// assert_eq!(to_snake_case("Foo__Bar"), "foo__bar");
/// ```
pub fn to_snake_case(name: &str) -> String {
    let mut snake = String::new();
    let mut prev = '_';
    for (i, char) in name.chars().enumerate() {
        if char.is_uppercase() {
            // characters without capitalization are considered lowercase
            if i != 0 && prev != '_' {
                snake.push('_');
            }
            snake.extend(char.to_lowercase());
        } else {
            snake.push(char);
        }
        prev = char
    }
    snake
}

// adapted from https://docs.rs/crate/human_bytes/latest
pub fn human_bytes(bytes: impl Into<f64>) -> String {
    const SUFFIX: [&str; 9] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    const UNIT: f64 = 1024.0;

    let size = bytes.into();

    if size <= 0.0 {
        return "0 B".to_string();
    }

    let base = size.log10() / UNIT.log10();

    let result = format!("{:.1}", UNIT.powf(base - base.floor()),)
        .trim_end_matches(".0")
        .to_owned();

    [&result, SUFFIX[base.floor() as usize]].join(" ")
}

// adapted from https://github.com/mitsuhiko/similar/blob/main/examples/terminal-inline.rs
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
