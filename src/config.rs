use {
    serde::Deserialize,
    std::{io, path::Path},
    thiserror::Error,
};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Config {
    pub case: Case,
    pub spaces: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            case: Case::Snake,
            spaces: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Case {
    #[serde(alias = "camelCase")]
    Camel,
    #[serde(alias = "snake_case")]
    Snake,
}

impl Config {
    pub fn from_path(path: &Path) -> Result<Config, ConfigError> {
        let mut current_path = path.to_path_buf();

        while current_path.pop() {
            let config_path = current_path.join("roughly.toml");
            if config_path.exists() {
                let text = std::fs::read_to_string(&config_path)?;
                return Ok(toml::from_str(&text)?);
            }
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
