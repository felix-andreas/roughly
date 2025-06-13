use {
    crate::lsp_types::{Position, Range},
    ropey::Rope,
    std::{fs::File, io::BufReader, path::Path},
    tree_sitter::Node,
};

pub fn starts_with_lowercase(name: &str, query: &str) -> bool {
    query.is_empty() || name.to_lowercase().starts_with(&query.to_lowercase())
}

pub fn read_to_rope(path: impl AsRef<Path>) -> std::io::Result<Rope> {
    Rope::from_reader(BufReader::new(File::open(path)?))
}

pub fn node_range(node: Node) -> Range {
    let start = node.start_position();
    let end = node.end_position();
    Range::new(
        Position::new(start.row as u32, start.column as u32),
        Position::new(end.row as u32, end.column as u32),
    )
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
