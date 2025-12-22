//! Feature flags for controlling optional behaviors.
//!
//! Features can be enabled via:
//! - CLI: `--features attempt-validation-fix,other-feature`
//! - Environment: `GIT_REABSORB_FEATURES=attempt-validation-fix,other-feature`

use std::collections::HashSet;
use std::env;
use std::sync::OnceLock;

use clap::ValueEnum;
use log::warn;
use serde::{Deserialize, Serialize};

/// Available feature flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Feature {
    /// Attempt to fix validation errors (unassigned/duplicate hunks) with targeted prompts
    /// instead of retrying from scratch.
    AttemptValidationFix,
    /// Use gitabsorb to absorb existing commits.
    Absorb,
}

impl Feature {
    /// Check if this feature is enabled in the global config.
    pub fn is_enabled(&self) -> bool {
        Features::global().is_enabled(*self)
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
                if let Ok(feature) = Feature::from_str(name, true) {
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
    pub fn with_overrides(mut self, cli_features: Option<&[Feature]>) -> Self {
        if let Some(features) = cli_features {
            for feature in features {
                self.enable(*feature);
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
            Feature::from_str("attempt-validation-fix", true).unwrap(),
            Feature::AttemptValidationFix
        );
        assert!(Feature::from_str("unknown", true).is_err());
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

        let features = features.with_overrides(Some(&[Feature::AttemptValidationFix]));
        assert!(features.is_enabled(Feature::AttemptValidationFix));
    }
}
