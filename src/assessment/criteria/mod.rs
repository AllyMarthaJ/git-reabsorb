//! Criterion definitions and trait for commit assessment.

pub mod atomicity;
pub mod cohesion;
pub mod message;
pub mod reversibility;
pub mod scope;

use serde::{Deserialize, Serialize};

use crate::assessment::types::{AssessmentLevel, CriterionScore};
use crate::models::SourceCommit;

/// Unique identifier for a criterion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionId {
    Atomicity,
    MessageQuality,
    LogicalCohesion,
    ScopeAppropriateness,
    Reversibility,
}

impl CriterionId {
    /// Returns all available criterion IDs.
    pub fn all() -> &'static [CriterionId] {
        &[
            CriterionId::Atomicity,
            CriterionId::MessageQuality,
            CriterionId::LogicalCohesion,
            CriterionId::ScopeAppropriateness,
            CriterionId::Reversibility,
        ]
    }

    /// Returns the human-readable name of this criterion.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Atomicity => "Atomicity",
            Self::MessageQuality => "Message Quality",
            Self::LogicalCohesion => "Logical Cohesion",
            Self::ScopeAppropriateness => "Scope Appropriateness",
            Self::Reversibility => "Reversibility",
        }
    }
}

impl std::fmt::Display for CriterionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Atomicity => write!(f, "atomicity"),
            Self::MessageQuality => write!(f, "message_quality"),
            Self::LogicalCohesion => write!(f, "logical_cohesion"),
            Self::ScopeAppropriateness => write!(f, "scope_appropriateness"),
            Self::Reversibility => write!(f, "reversibility"),
        }
    }
}

impl std::str::FromStr for CriterionId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "atomicity" => Ok(Self::Atomicity),
            "message_quality" | "message" => Ok(Self::MessageQuality),
            "logical_cohesion" | "cohesion" => Ok(Self::LogicalCohesion),
            "scope_appropriateness" | "scope" => Ok(Self::ScopeAppropriateness),
            "reversibility" => Ok(Self::Reversibility),
            _ => Err(format!("Unknown criterion: {}", s)),
        }
    }
}

/// Definition of a criterion with its rubric.
#[derive(Debug, Clone)]
pub struct CriterionDefinition {
    pub id: CriterionId,
    pub name: String,
    pub description: String,
    /// The 5 levels, sorted from 1 (worst) to 5 (best).
    pub levels: [AssessmentLevel; 5],
}

impl CriterionDefinition {
    /// Get the weight for a given level (1-5).
    pub fn weight_for_level(&self, level: u8) -> f32 {
        if (1..=5).contains(&level) {
            self.levels[(level - 1) as usize].weight
        } else {
            1.0
        }
    }

    /// Calculate the maximum possible weighted score.
    pub fn max_weighted_score(&self) -> f32 {
        5.0 * self.levels[4].weight
    }
}

/// Context about the commit range for assessment.
#[derive(Debug, Clone)]
pub struct RangeContext {
    /// All commits in the range (for understanding relationships).
    pub commits: Vec<SourceCommit>,
    /// Position of current commit (0-indexed).
    pub position: usize,
    /// Files changed across the entire range.
    pub files_in_range: Vec<String>,
    /// Previous assessments in this run (for consistency).
    pub prior_assessments: Vec<CriterionScore>,
}

impl RangeContext {
    pub fn new(commits: Vec<SourceCommit>, position: usize) -> Self {
        Self {
            commits,
            position,
            files_in_range: Vec::new(),
            prior_assessments: Vec::new(),
        }
    }

    pub fn with_files(mut self, files: Vec<String>) -> Self {
        self.files_in_range = files;
        self
    }

    pub fn with_prior_assessments(mut self, assessments: Vec<CriterionScore>) -> Self {
        self.prior_assessments = assessments;
        self
    }
}

/// Errors that can occur during assessment.
#[derive(Debug, thiserror::Error)]
pub enum AssessmentError {
    #[error("LLM assessment failed: {0}")]
    LlmFailed(String),
    #[error("Invalid criterion response: {0}")]
    InvalidResponse(String),
    #[error("Git operation failed: {0}")]
    GitError(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Trait for assessing commits against a criterion.
pub trait Criterion: Send + Sync {
    /// Get the criterion definition with rubric.
    fn definition(&self) -> &CriterionDefinition;

    /// Assess a single commit in context of the range.
    fn assess(
        &self,
        commit: &SourceCommit,
        diff_content: &str,
        range_context: &RangeContext,
    ) -> Result<CriterionScore, AssessmentError>;
}

/// Get the definition for a criterion by ID.
pub fn get_definition(id: CriterionId) -> CriterionDefinition {
    match id {
        CriterionId::Atomicity => atomicity::definition(),
        CriterionId::MessageQuality => message::definition(),
        CriterionId::LogicalCohesion => cohesion::definition(),
        CriterionId::ScopeAppropriateness => scope::definition(),
        CriterionId::Reversibility => reversibility::definition(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criterion_id_display() {
        assert_eq!(CriterionId::Atomicity.to_string(), "atomicity");
        assert_eq!(CriterionId::MessageQuality.to_string(), "message_quality");
    }

    #[test]
    fn criterion_id_parse() {
        assert_eq!(
            "atomicity".parse::<CriterionId>().unwrap(),
            CriterionId::Atomicity
        );
        assert_eq!(
            "message".parse::<CriterionId>().unwrap(),
            CriterionId::MessageQuality
        );
        assert_eq!(
            "cohesion".parse::<CriterionId>().unwrap(),
            CriterionId::LogicalCohesion
        );
    }

    #[test]
    fn all_criteria() {
        assert_eq!(CriterionId::all().len(), 5);
    }
}
