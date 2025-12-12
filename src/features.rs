//! Feature flags for controlling optional behaviors.
//!
//! Features can be enabled via:
//! - CLI: `--features attempt-validation-fix,other-feature`
//! - Environment: `GIT_REABSORB_FEATURES=attempt-validation-fix,other-feature`

use std::collections::HashSet;
use std::env;
use std::str::FromStr;
use std::sync::OnceLock;

use log::warn;

/// Available feature flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Attempt to fix validation errors (unassigned/duplicate hunks) with targeted prompts
    /// instead of retrying from scratch.
    AttemptValidationFix,
}

impl Feature {
    /// All available features.
    pub const ALL: &'static [Feature] = &[Feature::AttemptValidationFix];

    /// Get the CLI/env name for this feature.
    pub fn name(&self) -> &'static str {
        match self {
            Feature::AttemptValidationFix => "attempt-validation-fix",
        }
    }

    /// Get a description of what this feature does.
    pub fn description(&self) -> &'static str {
        match self {
            Feature::AttemptValidationFix => {
                "Fix validation errors with targeted prompts instead of full retry"
            }
        }
    }

    /// Check if this feature is enabled in the global config.
    pub fn is_enabled(&self) -> bool {
        Features::global().is_enabled(*self)
    }
}

impl std::fmt::Display for Feature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for Feature {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "attempt-validation-fix" => Ok(Feature::AttemptValidationFix),
            _ => Err(format!(
                "Unknown feature: '{}'. Available features: {}",
                s,
                Feature::ALL
                    .iter()
                    .map(|f| f.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

/// Collection of enabled features.
#[derive(Debug, Clone, Default)]
pub struct Features {
    enabled: HashSet<Feature>,
}

static GLOBAL_FEATURES: OnceLock<Features> = OnceLock::new();

impl Features {
    /// Create an empty feature set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create from environment variable.
    pub fn from_env() -> Self {
        let mut features = Self::new();
        if let Ok(value) = env::var("GIT_REABSORB_FEATURES") {
            for name in value.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                if let Ok(feature) = name.parse() {
                    features.enable(feature);
                } else {
                    warn!("Unknown feature '{}' in GIT_REABSORB_FEATURES", name);
                }
            }
        }
        features
    }

    /// Enable a feature.
    pub fn enable(&mut self, feature: Feature) {
        self.enabled.insert(feature);
    }

    /// Disable a feature.
    pub fn disable(&mut self, feature: Feature) {
        self.enabled.remove(&feature);
    }

    /// Check if a feature is enabled.
    pub fn is_enabled(&self, feature: Feature) -> bool {
        self.enabled.contains(&feature)
    }

    /// Merge with CLI overrides.
    pub fn with_overrides(mut self, cli_features: Option<&[String]>) -> Self {
        if let Some(features) = cli_features {
            for name in features {
                if let Ok(feature) = name.parse() {
                    self.enable(feature);
                } else {
                    warn!("Unknown feature '{}' in --features", name);
                }
            }
        }
        self
    }

    /// Get the global feature configuration.
    pub fn global() -> &'static Features {
        GLOBAL_FEATURES.get_or_init(Features::from_env)
    }

    /// Initialize the global feature configuration.
    /// Should be called once at startup with CLI overrides.
    pub fn init_global(features: Features) {
        let _ = GLOBAL_FEATURES.set(features);
    }

    /// List all enabled features.
    pub fn enabled_features(&self) -> impl Iterator<Item = Feature> + '_ {
        self.enabled.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature_parse() {
        assert_eq!(
            "attempt-validation-fix".parse::<Feature>().unwrap(),
            Feature::AttemptValidationFix
        );
        assert_eq!(
            "attempt_validation_fix".parse::<Feature>().unwrap(),
            Feature::AttemptValidationFix
        );
        assert!("unknown".parse::<Feature>().is_err());
    }

    #[test]
    fn test_features_enable_disable() {
        let mut features = Features::new();
        assert!(!features.is_enabled(Feature::AttemptValidationFix));

        features.enable(Feature::AttemptValidationFix);
        assert!(features.is_enabled(Feature::AttemptValidationFix));

        features.disable(Feature::AttemptValidationFix);
        assert!(!features.is_enabled(Feature::AttemptValidationFix));
    }

    #[test]
    fn test_features_with_overrides() {
        let features = Features::new();
        assert!(!features.is_enabled(Feature::AttemptValidationFix));

        let features = features.with_overrides(Some(&["attempt-validation-fix".to_string()]));
        assert!(features.is_enabled(Feature::AttemptValidationFix));
    }
}
