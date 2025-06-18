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

// Use a bitfield for efficient storage and copying
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExperimentalFeatures {
    flags: u8,
}

impl ExperimentalFeatures {
    const UNUSED: u8 = 1 << 0;
    const RANGE_FORMATTING: u8 = 1 << 1;

    pub fn new() -> Self {
        Self { flags: 0 }
    }

    pub fn from_strings(feature_strings: &[String]) -> (Self, Vec<String>) {
        let mut flags = 0;
        let mut unknown_features = Vec::new();

        for feature_str in feature_strings {
            if feature_str == "all" {
                flags |= Self::UNUSED | Self::RANGE_FORMATTING;
            } else {
                match ExperimentalFeature::from_str(feature_str) {
                    Ok(feature) => {
                        flags |= Self::feature_to_flag(feature);
                    }
                    Err(_) => {
                        unknown_features.push(feature_str.clone());
                    }
                }
            }
        }

        (Self { flags }, unknown_features)
    }

    fn feature_to_flag(feature: ExperimentalFeature) -> u8 {
        match feature {
            ExperimentalFeature::Unused => Self::UNUSED,
            ExperimentalFeature::RangeFormatting => Self::RANGE_FORMATTING,
        }
    }

    pub fn has(&self, feature: ExperimentalFeature) -> bool {
        (self.flags & Self::feature_to_flag(feature)) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.flags == 0
    }
}

impl Default for ExperimentalFeatures {
    fn default() -> Self {
        Self::new()
    }
}