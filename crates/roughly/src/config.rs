use {
    crate::format::Config as FormatConfig,
    analysis::{CheckConfig, LintConfig, NameStyle},
    serde::Deserialize,
    std::{io, path::Path},
    thiserror::Error,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct Config {
    pub format: FormatConfig,
    pub lint: LintConfig,
    pub check: CheckConfig,
}

impl Config {
    pub fn from_path(
        path: impl AsRef<Path>,
        experimental: ExperimentalFeatures,
    ) -> Result<Config, ConfigError> {
        let path = path.as_ref();

        Ok(match std::fs::read_to_string(path) {
            Ok(text) => Config::from_str(&text, experimental)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Config::default(),
            Err(error) => return Err(error.into()),
        })
    }

    pub fn from_str(text: &str, experimental: ExperimentalFeatures) -> Result<Config, ConfigError> {
        Ok(toml::from_str::<ConfigToml>(text)?.to_config(experimental))
    }

    /// Loads the `roughly.toml` governing `target` by searching the target's directory and
    /// its ancestors, falling back to the default configuration when none exists.
    pub fn for_target(
        target: impl AsRef<Path>,
        experimental: ExperimentalFeatures,
    ) -> Result<Config, ConfigError> {
        let target = target.as_ref();
        let start = if target.is_dir() {
            Some(target)
        } else {
            target.parent()
        };

        let mut directory = start;
        while let Some(current) = directory {
            let candidate = current.join("roughly.toml");
            if candidate.is_file() {
                return Config::from_path(candidate, experimental);
            }
            directory = current.parent();
        }

        Ok(Config::default())
    }
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("failed to read config")]
    IoError(#[from] io::Error),
    #[error("invalid config file")]
    Invalid(#[from] toml::de::Error),
}

//
// TOML
//

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ConfigToml {
    pub case: Option<NameStyle>, // kept for backwards compatibility
    pub spaces: Option<usize>,   // kept for backwards compatibility
    pub format: FormatConfig,
    pub lint: LintConfig,
    pub check: CheckConfig,
}

impl ConfigToml {
    pub fn to_config(mut self, experimental: ExperimentalFeatures) -> Config {
        if let Some(spaces) = self.spaces {
            self.format.indent_width = spaces;
        }

        if let Some(case) = self.case {
            self.lint.naming_style = Some(case);
        }

        self.check.unused |= experimental.unused;
        self.check.typing |= experimental.typing;

        Config {
            format: self.format,
            lint: self.lint,
            check: self.check,
        }
    }
}

//
// EXPERIMENTAL FEATURES
//

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExperimentalFeatures {
    pub goto_references: bool,
    pub hovering: bool,
    pub debug: bool,
    pub range_formatting: bool,
    pub rename: bool,
    pub unused: bool,
    pub typing: bool,
}

#[cfg(test)]
mod tests {
    use {super::*, crate::format::LineEnding, indoc::indoc};
    fn parse(text: &str) -> Config {
        Config::from_str(text, ExperimentalFeatures::default()).unwrap()
    }

    #[test]
    fn all_fields() {
        let toml = indoc! {r#"
            [format]
            indent-width = 4
            line-ending = "auto"

            [lint]
            naming-style = "snake_case"
            experimental-unused = true
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
    }
}
