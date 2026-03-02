//! Criterion definitions and trait for commit assessment.

pub mod coherence;
pub mod message;
pub mod scope;
pub mod self_containment;

use serde::{Deserialize, Serialize};

use crate::assessment::types::{AssessmentLevel, CriterionScore};
use crate::models::SourceCommit;

/// Unique identifier for a criterion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionId {
    Coherence,
    MessageQuality,
    SelfContainment,
    ScopeAppropriateness,
}

impl CriterionId {
    /// Returns the LLM-assessed criterion IDs (excludes Scope, which is deterministic).
    pub fn llm_assessed() -> &'static [CriterionId] {
        &[
            CriterionId::Coherence,
            CriterionId::MessageQuality,
            CriterionId::SelfContainment,
        ]
    }

    /// Returns all criterion IDs including deterministic ones.
    pub fn all() -> &'static [CriterionId] {
        &[
            CriterionId::Coherence,
            CriterionId::MessageQuality,
            CriterionId::SelfContainment,
            CriterionId::ScopeAppropriateness,
        ]
    }

    /// Returns the human-readable name of this criterion.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Coherence => "Coherence",
            Self::MessageQuality => "Message Quality",
            Self::SelfContainment => "Self-Containment",
            Self::ScopeAppropriateness => "Scope Appropriateness",
        }
    }
}

impl std::fmt::Display for CriterionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Coherence => write!(f, "coherence"),
            Self::MessageQuality => write!(f, "message_quality"),
            Self::SelfContainment => write!(f, "self_containment"),
            Self::ScopeAppropriateness => write!(f, "scope_appropriateness"),
        }
    }
}

impl std::str::FromStr for CriterionId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "coherence" | "atomicity" | "cohesion" | "logical_cohesion" => Ok(Self::Coherence),
            "message_quality" | "message" => Ok(Self::MessageQuality),
            "self_containment" | "reversibility" => Ok(Self::SelfContainment),
            "scope_appropriateness" | "scope" => Ok(Self::ScopeAppropriateness),
            _ => Err(format!("Unknown criterion: {}", s)),
        }
    }
}

/// Definition of a criterion with its rubric.
#[derive(Debug, Clone)]
pub struct CriterionDefinition {
    pub id: CriterionId,
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

/// Diff statistics for deterministic scope scoring.
#[derive(Debug, Clone)]
pub struct DiffStats {
    pub lines_added: usize,
    pub lines_removed: usize,
    pub files_changed: usize,
}

impl DiffStats {
    pub fn total_lines(&self) -> usize {
        self.lines_added + self.lines_removed
    }
}

/// Compute a deterministic scope score from diff statistics.
pub fn compute_scope_score(stats: &DiffStats) -> CriterionScore {
    let def = scope::definition();
    let total = stats.total_lines();

    let level: u8 = if total == 0 || total > 800 {
        1
    } else if total > 400 || (stats.files_changed > 20 && total > 200) {
        2
    } else if total > 200 {
        3
    } else if total > 30 {
        4
    } else {
        5
    };

    let weight = def.weight_for_level(level);

    let rationale = match level {
        1 if total == 0 => "Empty diff (whitespace-only or no changes)".to_string(),
        1 => format!("{} lines changed — too large for effective review", total),
        2 => format!("{} lines across {} files — should be split", total, stats.files_changed),
        3 => format!("{} lines — reviewable but could be split", total),
        4 => format!("{} lines — good size for review", total),
        5 => format!("{} lines — ideal, focused change", total),
        _ => unreachable!(),
    };

    CriterionScore {
        criterion_id: CriterionId::ScopeAppropriateness,
        level,
        weighted_score: level as f32 * weight,
        rationale,
        evidence: vec![
            format!("{} lines added", stats.lines_added),
            format!("{} lines removed", stats.lines_removed),
            format!("{} files changed", stats.files_changed),
        ],
        suggestions: Vec::new(),
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

/// Get the definition for a criterion by ID.
pub fn get_definition(id: CriterionId) -> CriterionDefinition {
    match id {
        CriterionId::Coherence => coherence::definition(),
        CriterionId::MessageQuality => message::definition(),
        CriterionId::SelfContainment => self_containment::definition(),
        CriterionId::ScopeAppropriateness => scope::definition(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criterion_id_display() {
        assert_eq!(CriterionId::Coherence.to_string(), "coherence");
        assert_eq!(CriterionId::MessageQuality.to_string(), "message_quality");
        assert_eq!(
            CriterionId::SelfContainment.to_string(),
            "self_containment"
        );
    }

    #[test]
    fn criterion_id_parse() {
        assert_eq!(
            "coherence".parse::<CriterionId>().unwrap(),
            CriterionId::Coherence
        );
        assert_eq!(
            "message".parse::<CriterionId>().unwrap(),
            CriterionId::MessageQuality
        );
        assert_eq!(
            "self_containment".parse::<CriterionId>().unwrap(),
            CriterionId::SelfContainment
        );
    }

    #[test]
    fn backward_compat_aliases() {
        assert_eq!(
            "atomicity".parse::<CriterionId>().unwrap(),
            CriterionId::Coherence
        );
        assert_eq!(
            "cohesion".parse::<CriterionId>().unwrap(),
            CriterionId::Coherence
        );
        assert_eq!(
            "logical_cohesion".parse::<CriterionId>().unwrap(),
            CriterionId::Coherence
        );
        assert_eq!(
            "reversibility".parse::<CriterionId>().unwrap(),
            CriterionId::SelfContainment
        );
    }

    #[test]
    fn all_criteria() {
        assert_eq!(CriterionId::all().len(), 4);
    }

    #[test]
    fn llm_assessed_excludes_scope() {
        let llm = CriterionId::llm_assessed();
        assert_eq!(llm.len(), 3);
        assert!(!llm.contains(&CriterionId::ScopeAppropriateness));
    }

    #[test]
    fn compute_scope_ideal() {
        let stats = DiffStats {
            lines_added: 10,
            lines_removed: 5,
            files_changed: 2,
        };
        let score = compute_scope_score(&stats);
        assert_eq!(score.level, 5);
    }

    #[test]
    fn compute_scope_good() {
        let stats = DiffStats {
            lines_added: 100,
            lines_removed: 50,
            files_changed: 4,
        };
        let score = compute_scope_score(&stats);
        assert_eq!(score.level, 4);
    }

    #[test]
    fn compute_scope_too_large() {
        let stats = DiffStats {
            lines_added: 500,
            lines_removed: 400,
            files_changed: 15,
        };
        let score = compute_scope_score(&stats);
        assert_eq!(score.level, 1);
    }

    #[test]
    fn compute_scope_empty() {
        let stats = DiffStats {
            lines_added: 0,
            lines_removed: 0,
            files_changed: 0,
        };
        let score = compute_scope_score(&stats);
        assert_eq!(score.level, 1);
    }

    #[test]
    fn compute_scope_many_files_cap() {
        let stats = DiffStats {
            lines_added: 150,
            lines_removed: 100,
            files_changed: 25,
        };
        let score = compute_scope_score(&stats);
        assert_eq!(score.level, 2); // capped due to files > 20 && total > 200
    }
}
