use {
    crate::format::Config as FormatConfig,
    analysis::{CheckConfig, LintConfig, NameStyle},
    serde::Deserialize,
    std::{
        fmt, io,
        path::{Path, PathBuf},
    },
    thiserror::Error,
};

pub const CONFIG_FILE_NAME: &str = "roughly.toml";

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Config {
    pub format: FormatConfig,
    pub lint: LintConfig,
    pub check: CheckConfig,
    // Surfaces internal analysis facts (the hover debug sections). A developer aid, off by default.
    pub debug: bool,
}

impl Config {
    /// Loads the `roughly.toml` governing `target`: the nearest one found in the target's
    /// directory (its parent, when `target` is a file) or the directory's ancestors, falling back
    /// to the default configuration when none exists. Both the language server (from the client's
    /// workspace root) and the CLI (from each target argument) resolve configuration through this
    /// one search.
    pub fn discover(target: impl AsRef<Path>) -> Result<Config, ConfigError> {
        match find_config_file(target.as_ref())? {
            Some(path) => Config::from_path(path),
            None => Ok(Config::default()),
        }
    }

    /// The path of the config file [`discover`](Config::discover) would load for `target`, when one
    /// exists — the file the language server must watch for live reloads (it may sit in an ancestor
    /// *above* the workspace root). Resolution failures degrade to `None`: the caller is deciding
    /// what to watch, and `discover` itself reports the error.
    pub fn discover_path(target: impl AsRef<Path>) -> Option<PathBuf> {
        find_config_file(target.as_ref()).ok().flatten()
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
        let path = path.as_ref();

        match std::fs::read_to_string(path) {
            Ok(text) => Config::from_toml_str(&text).map_err(|error| error.with_path(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(error) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source: error,
            }),
        }
    }

    pub fn from_toml_str(text: &str) -> Result<Config, ConfigError> {
        match toml::from_str::<ConfigToml>(text) {
            Ok(config) => Ok(config.to_config()),
            Err(error) => Err(ConfigError::Invalid(ConfigParseError::new(&error, text))),
        }
    }
}

// The nearest-ancestor search both `discover` and `discover_path` walk: the target's own directory
// (its parent when the target is a file), then each ancestor, for a `roughly.toml`.
fn find_config_file(target: &Path) -> Result<Option<PathBuf>, ConfigError> {
    // A relative path is made absolute first: its lexical parent chain ends at the empty path,
    // so walking it directly would never reach the real filesystem ancestors.
    let target = std::path::absolute(target).map_err(|error| ConfigError::Resolve {
        path: target.to_path_buf(),
        source: error,
    })?;
    // `std::path::absolute` keeps `..` components, and `parent()` strips them lexically — so a
    // target like `/a/b/../c.R` would walk `/a/b/..` and then back INTO `/a/b`, which is not an
    // ancestor of the target at all. Resolve `.`/`..` away first so the walk visits only true
    // ancestors.
    let target = normalize_lexically(&target);

    let mut directory = if target.is_dir() {
        Some(target.as_path())
    } else {
        target.parent()
    };
    while let Some(current) = directory {
        let candidate = current.join(CONFIG_FILE_NAME);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
        directory = current.parent();
    }
    Ok(None)
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}", path = .path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("failed to resolve {path} while searching for a config file: {source}", path = .path.display())]
    Resolve { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Invalid(ConfigParseError),
}

impl ConfigError {
    fn with_path(mut self, path: &Path) -> ConfigError {
        if let ConfigError::Invalid(error) = &mut self {
            error.path = Some(path.to_path_buf());
        }
        self
    }

    /// The 1-based line and column of a parse or deserialize failure inside the config text, when
    /// the underlying toml error carries a span. `None` for I/O and resolution failures.
    pub fn parse_location(&self) -> Option<(usize, usize)> {
        match self {
            ConfigError::Invalid(error) => error.location,
            ConfigError::Io { .. } | ConfigError::Resolve { .. } => None,
        }
    }
}

/// A malformed `roughly.toml`: the toml parse or deserialize failure, with its source span
/// resolved to a 1-based line and column so the message can point at the offending key or value.
#[derive(Debug)]
pub struct ConfigParseError {
    path: Option<PathBuf>,
    location: Option<(usize, usize)>,
    message: String,
}

impl ConfigParseError {
    fn new(error: &toml::de::Error, text: &str) -> ConfigParseError {
        ConfigParseError {
            path: None,
            location: error.span().map(|span| line_and_column(text, span.start)),
            message: error.message().to_owned(),
        }
    }
}

impl fmt::Display for ConfigParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid config")?;
        if let Some(path) = &self.path {
            write!(formatter, " in {}", path.display())?;
        }
        if let Some((line, column)) = self.location {
            write!(formatter, " at line {line}, column {column}")?;
        }
        write!(formatter, ": {}", self.message)
    }
}

impl std::error::Error for ConfigParseError {}

// Resolves `.` and `..` components lexically (without touching the filesystem), so an ancestor
// walk over the result visits only true ancestors. Lexical resolution can differ from the
// filesystem view when `..` crosses a symlink; the walk prefers the path as the user spelled it,
// which matches how sibling tools resolve project roots.
fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                // The root's parent is the root itself, so a `..` there dissolves.
                Some(Component::RootDir | Component::Prefix(_)) => {}
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                // A leading `..` on a relative path has nothing to cancel and must survive.
                _ => normalized.push(component),
            },
            other => normalized.push(other),
        }
    }
    normalized
}

// The 1-based line and column of a byte offset within `text`, for rendering a toml error span.
fn line_and_column(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let line_start = text[..offset].rfind('\n').map_or(0, |newline| newline + 1);
    let line = text[..line_start].matches('\n').count() + 1;
    let column = text[line_start..offset].chars().count() + 1;
    (line, column)
}

//
// TOML
//

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct ConfigToml {
    pub case: Option<NameStyle>, // kept for backwards compatibility
    pub spaces: Option<usize>,   // kept for backwards compatibility
    pub format: FormatConfig,
    pub lint: LintConfig,
    pub check: CheckConfig,
    pub debug: bool,
}

impl ConfigToml {
    pub fn to_config(mut self) -> Config {
        if let Some(spaces) = self.spaces {
            self.format.indent_width = spaces;
        }

        if let Some(case) = self.case {
            self.lint.naming_style = Some(case);
        }

        Config {
            format: self.format,
            lint: self.lint,
            check: self.check,
            debug: self.debug,
        }
    }
}

//
// EXPERIMENTAL FEATURES
//

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExperimentalFeatures {
    pub range_formatting: bool,
}

#[cfg(test)]
mod tests {
    use {super::*, crate::format::LineEnding, indoc::indoc};
    fn parse(text: &str) -> Config {
        Config::from_toml_str(text).unwrap()
    }

    fn parse_error(text: &str) -> String {
        Config::from_toml_str(text)
            .expect_err("expected the config to be rejected")
            .to_string()
    }

    #[test]
    fn per_lint_levels() {
        let config = parse(indoc! {r#"
            [lint]
            assignment-operator = "off"
            boolean-shorthand = "error"
        "#});
        assert_eq!(config.lint.assignment_operator, analysis::LintLevel::Off);
        assert_eq!(config.lint.boolean_shorthand, analysis::LintLevel::Error);
        assert_eq!(config.lint.missing_comma, analysis::LintLevel::Default);
    }

    #[test]
    fn all_fields() {
        let toml = indoc! {r#"
            [format]
            indent-width = 4
            line-ending = "auto"

            [lint]
            naming-style = "snake_case"
        "#};
        let config = parse(toml);
        assert_eq!(config.format.indent_width, 4);
        assert_eq!(config.format.line_ending, LineEnding::Auto);
        assert_eq!(config.lint.naming_style, Some(NameStyle::Snake));
    }

    #[test]
    fn backwards_compatability() {
        let toml = indoc! {r#"
            case = "snake_case" # should override lint.naming-style
            spaces = 6          # should override format.indent-width

            [format]
            indent-width = 4
            line-ending = "cr-lf"

            [lint]
            naming-style = "camelCase"

            [check]
            unused = true
        "#};
        let config = parse(toml);
        assert_eq!(config.format.indent_width, 6);
        assert_eq!(config.format.line_ending, LineEnding::CrLf);
        assert_eq!(config.lint.naming_style, Some(NameStyle::Snake));
        assert!(config.check.unused);
    }

    #[test]
    fn defaults() {
        let toml = indoc! {r#"
            [format]
            [lint]
        "#};
        let config = parse(toml);
        assert_eq!(config.format.indent_width, 2);
        assert_eq!(config.format.line_ending, LineEnding::Auto);
        assert_eq!(config.lint.naming_style, None);
        assert!(!config.check.unused);
        assert!(!config.check.typing);
        assert!(!config.debug);
    }

    #[test]
    fn debug_toggle() {
        assert!(parse("debug = true\n").debug);
        assert!(!parse("debug = false\n").debug);
    }

    #[test]
    fn unknown_top_level_key_is_rejected_with_location() {
        let message = parse_error("strict = true\n");
        assert!(
            message.contains("unknown field `strict`"),
            "expected the error to name the unknown key, got: {message}"
        );
        assert!(
            message.contains("at line 1, column 1"),
            "expected the error to point at the key, got: {message}"
        );
    }

    #[test]
    fn unknown_key_in_each_section_is_rejected() {
        let format = parse_error("[format]\nindent = 4\n");
        assert!(
            format.contains("unknown field `indent`") && format.contains("line 2"),
            "expected an unknown-field error in [format], got: {format}"
        );

        let lint = parse_error("[lint]\nstyle = \"snake_case\"\n");
        assert!(
            lint.contains("unknown field `style`") && lint.contains("line 2"),
            "expected an unknown-field error in [lint], got: {lint}"
        );

        let check = parse_error("[check]\ntyping = true\nstric = true\n");
        assert!(
            check.contains("unknown field `stric`") && check.contains("line 3"),
            "expected an unknown-field error in [check], got: {check}"
        );
    }

    #[test]
    fn underscore_spelling_of_kebab_case_key_is_rejected() {
        let message = parse_error("[lint]\nnaming_style = \"snake_case\"\n");
        assert!(
            message.contains("unknown field `naming_style`"),
            "expected the error to name the misspelled key, got: {message}"
        );
        assert!(
            message.contains("`naming-style`"),
            "expected the error to suggest the kebab-case spelling, got: {message}"
        );
        assert!(
            message.contains("at line 2, column 1"),
            "expected the error to point at the key, got: {message}"
        );
    }

    #[test]
    fn wrong_value_type_is_rejected_with_location() {
        let message = parse_error("[check]\nstrict = \"yes\"\n");
        assert!(
            message.contains("expected a boolean"),
            "expected a type error, got: {message}"
        );
        assert!(
            message.contains("at line 2"),
            "expected the error to point at the offending line, got: {message}"
        );
    }

    #[test]
    fn from_path_names_the_file_in_errors() {
        let directory = tempfile::tempdir().expect("temp dir");
        let config_path = directory.path().join(CONFIG_FILE_NAME);
        std::fs::write(&config_path, "debug = 1\n").expect("write config");

        let message = Config::from_path(&config_path)
            .expect_err("expected the config to be rejected")
            .to_string();
        assert!(
            message.contains("roughly.toml"),
            "expected the error to name the file, got: {message}"
        );
        assert!(
            message.contains("at line 1"),
            "expected the error to carry a location, got: {message}"
        );
    }

    #[test]
    fn discover_walks_ancestors_from_a_file_target() {
        let directory = tempfile::tempdir().expect("temp dir");
        let root = directory.path();
        std::fs::write(root.join(CONFIG_FILE_NAME), "[format]\nindent-width = 7\n")
            .expect("write config");
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).expect("create nested dirs");
        std::fs::write(nested.join("script.R"), "x <- 1\n").expect("write script");

        let config = Config::discover(nested.join("script.R")).expect("discover config");
        assert_eq!(config.format.indent_width, 7);
    }

    #[test]
    fn discover_prefers_the_nearest_config() {
        let directory = tempfile::tempdir().expect("temp dir");
        let root = directory.path();
        std::fs::write(root.join(CONFIG_FILE_NAME), "[format]\nindent-width = 7\n")
            .expect("write outer config");
        let nested = root.join("inner");
        std::fs::create_dir_all(&nested).expect("create nested dir");
        std::fs::write(
            nested.join(CONFIG_FILE_NAME),
            "[format]\nindent-width = 3\n",
        )
        .expect("write inner config");

        let from_directory = Config::discover(&nested).expect("discover from directory");
        assert_eq!(from_directory.format.indent_width, 3);

        let from_file = Config::discover(nested.join("script.R")).expect("discover from file");
        assert_eq!(from_file.format.indent_width, 3);
    }

    #[test]
    fn discover_defaults_when_no_config_exists() {
        let directory = tempfile::tempdir().expect("temp dir");
        let config = Config::discover(directory.path()).expect("discover config");
        assert_eq!(config, Config::default());
    }

    // A `..` in the target must not let the walk wander back into a sibling directory: a config in
    // `project/` does not govern `project/../outside.R`.
    #[test]
    fn discover_ignores_configs_behind_dot_dot_components() {
        let directory = tempfile::tempdir().expect("temp dir");
        let root = directory.path();
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("create project dir");
        std::fs::write(
            project.join(CONFIG_FILE_NAME),
            "[format]\nindent-width = 7\n",
        )
        .expect("write project config");
        std::fs::write(root.join("outside.R"), "x <- 1\n").expect("write outside script");

        let config = Config::discover(project.join("..").join("outside.R"))
            .expect("discover through dot-dot");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn normalize_lexically_resolves_dot_components() {
        use std::path::Path;
        assert_eq!(
            super::normalize_lexically(Path::new("/a/b/../c/./d.R")),
            Path::new("/a/c/d.R")
        );
        assert_eq!(
            super::normalize_lexically(Path::new("/../a")),
            Path::new("/a")
        );
        assert_eq!(
            super::normalize_lexically(Path::new("../../a")),
            Path::new("../../a")
        );
    }
}
