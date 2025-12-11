//! Core types for commit assessment.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single level within a criterion's rubric (1-5 scale).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssessmentLevel {
    /// Score from 1-5.
    pub score: u8,
    /// Weight factor for this level (e.g., 1.0 for standard, 1.2 for critical).
    pub weight: f32,
    /// Human-readable description of what this level means.
    pub description: String,
    /// Example indicators that suggest this level.
    pub indicators: Vec<String>,
}

impl AssessmentLevel {
    pub fn new(score: u8, weight: f32, description: impl Into<String>) -> Self {
        Self {
            score,
            weight,
            description: description.into(),
            indicators: Vec::new(),
        }
    }

    pub fn with_indicators(mut self, indicators: Vec<String>) -> Self {
        self.indicators = indicators;
        self
    }
}

/// Result of assessing a single criterion for a single commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionScore {
    /// The criterion that was assessed.
    pub criterion_id: String,
    /// The level achieved (1-5).
    pub level: u8,
    /// Weighted score (level * weight).
    pub weighted_score: f32,
    /// Explanation for why this level was assigned.
    pub rationale: String,
    /// Specific evidence from the commit.
    pub evidence: Vec<String>,
    /// Suggestions for improvement.
    pub suggestions: Vec<String>,
}

/// Complete assessment of a single commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitAssessment {
    /// SHA of the commit being assessed.
    pub commit_sha: String,
    /// Short message of the commit.
    pub commit_message: String,
    /// Individual criterion scores.
    pub criterion_scores: Vec<CriterionScore>,
    /// Overall weighted score (0.0 to 1.0).
    pub overall_score: f32,
    /// Position in the commit range (0-indexed).
    pub position: usize,
    /// Total commits in range.
    pub total_commits: usize,
}

/// Aggregate statistics for a criterion across the range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateScore {
    pub criterion_id: String,
    pub criterion_name: String,
    pub mean_score: f32,
    pub min_score: f32,
    pub max_score: f32,
    pub std_deviation: f32,
}

/// Assessment of an entire commit range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeAssessment {
    /// Base commit (exclusive).
    pub base_sha: String,
    /// Head commit (inclusive).
    pub head_sha: String,
    /// Timestamp of assessment (RFC 3339 format).
    pub assessed_at: String,
    /// Individual commit assessments.
    pub commit_assessments: Vec<CommitAssessment>,
    /// Aggregate scores by criterion.
    pub aggregate_scores: HashMap<String, AggregateScore>,
    /// Overall range score (0.0 to 1.0).
    pub overall_score: f32,
    /// Range-level observations.
    pub range_observations: Vec<String>,
}

/// Comparison between two assessments (before/after).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssessmentComparison {
    pub before: RangeAssessment,
    pub after: RangeAssessment,
    /// Change in overall score.
    pub overall_delta: f32,
    /// Per-criterion deltas (positive = improvement).
    pub criterion_deltas: HashMap<String, f32>,
    /// Summary of improvements.
    pub improvements: Vec<String>,
    /// Summary of regressions.
    pub regressions: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assessment_level_construction() {
        let level = AssessmentLevel::new(3, 1.0, "Adequate").with_indicators(vec![
            "Some context provided".to_string(),
            "Partially explains why".to_string(),
        ]);

        assert_eq!(level.score, 3);
        assert_eq!(level.weight, 1.0);
        assert_eq!(level.indicators.len(), 2);
    }

    #[test]
    fn criterion_score_serialization() {
        let score = CriterionScore {
            criterion_id: "atomicity".to_string(),
            level: 4,
            weighted_score: 4.0,
            rationale: "Single logical change".to_string(),
            evidence: vec!["Only touches auth module".to_string()],
            suggestions: vec![],
        };

        let json = serde_json::to_string(&score).unwrap();
        let restored: CriterionScore = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.level, 4);
    }
}
