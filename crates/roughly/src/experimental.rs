use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExperimentalFeature {
    Unused,
    RangeFormatting,
}

impl FromStr for ExperimentalFeature {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "unused" => Ok(ExperimentalFeature::Unused),
            "range_formatting" => Ok(ExperimentalFeature::RangeFormatting),
            _ => Err(format!("unknown experimental feature: {}", s)),
        }
    }
}

impl ExperimentalFeature {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExperimentalFeature::Unused => "unused",
            ExperimentalFeature::RangeFormatting => "range_formatting",
        }
    }

    pub fn all() -> Vec<ExperimentalFeature> {
        vec![
            ExperimentalFeature::Unused,
            ExperimentalFeature::RangeFormatting,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExperimentalFeatures {
    pub unused: bool,
    pub range_formatting: bool,
}

impl ExperimentalFeatures {
    pub fn new() -> Self {
        Self {
            unused: false,
            range_formatting: false,
        }
    }

    pub fn from_strings(feature_strings: &[String]) -> (Self, Vec<String>) {
        let mut features = Self::new();
        let mut unknown_features = Vec::new();

        for feature_str in feature_strings {
            if feature_str == "all" {
                features.unused = true;
                features.range_formatting = true;
            } else {
                match ExperimentalFeature::from_str(feature_str) {
                    Ok(feature) => {
                        match feature {
                            ExperimentalFeature::Unused => features.unused = true,
                            ExperimentalFeature::RangeFormatting => features.range_formatting = true,
                        }
                    }
                    Err(_) => {
                        unknown_features.push(feature_str.clone());
                    }
                }
            }
        }

        (features, unknown_features)
    }

    pub fn has(&self, feature: ExperimentalFeature) -> bool {
        match feature {
            ExperimentalFeature::Unused => self.unused,
            ExperimentalFeature::RangeFormatting => self.range_formatting,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.unused && !self.range_formatting
    }
}

impl Default for ExperimentalFeatures {
    fn default() -> Self {
        Self::new()
    }
}
