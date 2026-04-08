use {
    crate::format::Config as FormatConfig,
    analysis::{LintConfig as AnalysisLintConfig, NameStyle},
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
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct LintConfig {
    #[serde(flatten)]
    pub analysis: AnalysisLintConfig,
    pub experimental_unused: bool,
    pub experimental_typing: bool,
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
}

impl ConfigToml {
    pub fn to_config(mut self, experimental: ExperimentalFeatures) -> Config {
        if let Some(spaces) = self.spaces {
            self.format.indent_width = spaces;
        }

        if let Some(case) = self.case {
            self.lint.analysis.naming_style = Some(case);
        }

        self.lint.experimental_unused |= experimental.unused;
        self.lint.experimental_typing |= experimental.typing;

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
        assert_eq!(config.lint.analysis.naming_style, Some(NameStyle::Snake));
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
        assert_eq!(config.lint.analysis.naming_style, Some(NameStyle::Snake));
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
        assert_eq!(config.lint.analysis.naming_style, None);
    }
}
