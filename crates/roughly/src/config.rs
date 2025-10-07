use {
    crate::{diagnostics::Config as LintConfig, format::Config as FormatConfig},
    serde::Deserialize,
    std::{io, path::Path},
    thiserror::Error,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct Config {
    pub format: FormatConfig,
    pub lint: LintConfig,
}

impl Config {
    pub fn from_path(
        path: &Path,
        experimental: ExperimentalFeatures,
    ) -> Result<Config, ConfigError> {
        let config_path = path
            .ancestors()
            .map(|path| path.join("roughly.toml"))
            .find(|path| path.exists());

        Ok(match config_path {
            Some(config_path) => {
                let text = std::fs::read_to_string(config_path)?;
                Config::from_str(&text, experimental)?
            }
            None => Config::default(),
        })
    }

    pub fn from_str(text: &str, experimental: ExperimentalFeatures) -> Result<Config, ConfigError> {
        Ok(toml::from_str::<ConfigToml>(text)?.to_config(experimental))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Case {
    #[serde(alias = "camelCase")]
    Camel,
    #[serde(alias = "snake_case")]
    Snake,
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
    pub case: Option<Case>,    // kept for backwards compatibility
    pub spaces: Option<usize>, // kept for backwards compatibility
    pub format: FormatConfig,
    pub lint: LintConfig,
}

impl ConfigToml {
    pub fn to_config(mut self, experimental: ExperimentalFeatures) -> Config {
        if let Some(spaces) = self.spaces {
            self.format.indent_width = spaces;
        }

        if let Some(case) = self.case {
            self.lint.naming_style = Some(case);
        }

        self.lint.experimental_unused |= experimental.unused;

        Config {
            format: self.format,
            lint: self.lint,
        }
    }
}

//
// EXPERIMENTAL FEATURES
//

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExperimentalFeatures {
    pub goto_references: bool,
    pub range_formatting: bool,
    pub rename: bool,
    pub unused: bool,
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
        assert_eq!(config.lint.naming_style, Some(Case::Snake));
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
            experimental-unused = true
        "#};
        let config = parse(toml);
        assert_eq!(config.format.indent_width, 6);
        assert_eq!(config.format.line_ending, LineEnding::CrLf);
        assert_eq!(config.lint.naming_style, Some(Case::Snake));
        assert!(config.lint.experimental_unused);
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
    }
}
