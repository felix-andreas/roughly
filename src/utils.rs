use {
    crate::lsp_types::{Position, Range},
    ropey::Rope,
    std::{
        borrow::Cow,
        path::{Path, PathBuf},
        str::FromStr,
    },
    tower_lsp_server::lsp_types::Uri,
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

// from https://github.com/Desdaemon/odoo-lsp/blob/9c03a1219f01a54bbdee76db4f012856cf72d90b/src/utils.rs#L486-L533
// https://github.com/gluon-lang/lsp-types/issues/284#issuecomment-2147394262
// https://github.com/tower-lsp-community/tower-lsp-server/issues/34
pub trait UriExt {
    fn to_file_path(&self) -> Cow<Path>;
}

impl UriExt for tower_lsp_server::lsp_types::Uri {
    fn to_file_path(&self) -> Cow<Path> {
        let path = match self.path().as_estr().decode().into_string_lossy() {
            Cow::Borrowed(ref_) => Cow::Borrowed(Path::new(ref_)),
            Cow::Owned(owned) => Cow::Owned(PathBuf::from(owned)),
        };

        #[cfg(windows)]
        {
            let authority = self.authority().expect("url has no authority component");
            let host = authority.host().as_str();
            if host.is_empty() {
                // very high chance this is a `file:///` uri
                // in which case the path will include a leading slash we need to remove
                let host = path.to_string_lossy();
                let host = &host[1..];
                return Some(Cow::Owned(PathBuf::from(host)));
            }

            let host = format!("{host}:");
            Cow::Owned(
                Path::new(&host)
                    .components()
                    .chain(path.components())
                    .collect(),
            )
        }

        #[cfg(not(windows))]
        path
    }
}

pub fn uri_from_file_path(path: &Path) -> Option<Uri> {
    let fragment = if !path.is_absolute() {
        Cow::from(strict_canonicalize(path).ok()?)
    } else {
        Cow::from(path)
    };

    #[cfg(windows)]
    {
        // we want to parse a triple-slash path for Windows paths
        // it's a shorthand for `file://localhost/C:/Windows` with the `localhost` omitted
        let raw = format!("file:///{}", fragment.to_string_lossy().replace("\\", "/"));
        Uri::from_str(&raw).ok()
    }

    #[cfg(not(windows))]
    Uri::from_str(&format!("file://{}", fragment.to_string_lossy())).ok()
}

#[cfg(not(windows))]
pub use std::fs::canonicalize as strict_canonicalize;

/// On Windows, rewrites the wide path prefix `\\?\C:` to `C:`
/// Source: https://stackoverflow.com/a/70970317
#[inline]
#[cfg(windows)]
pub fn strict_canonicalize<P: AsRef<Path>>(path: P) -> anyhow::Result<PathBuf> {
    use anyhow::Context;

    fn impl_(path: PathBuf) -> anyhow::Result<PathBuf> {
        let head = path.components().next().context("empty path")?;
        let disk_;
        let head = if let std::path::Component::Prefix(prefix) = head {
            if let std::path::Prefix::VerbatimDisk(disk) = prefix.kind() {
                disk_ = format!("{}:", disk as char);
                Path::new(&disk_)
                    .components()
                    .next()
                    .context("failed to parse disk component")?
            } else {
                head
            }
        } else {
            head
        };
        Ok(std::iter::once(head)
            .chain(path.components().skip(1))
            .collect())
    }
    let canon = std::fs::canonicalize(path)?;
    impl_(canon)
}
