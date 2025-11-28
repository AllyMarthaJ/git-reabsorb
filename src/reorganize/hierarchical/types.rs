//! Types for hierarchical reorganization

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::models::HunkId;

/// Category of a code change
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeCategory {
    Feature,
    Bugfix,
    Refactor,
    Test,
    Documentation,
    Configuration,
    Dependency,
    Formatting,
    /// Fallback for unrecognized categories
    Other,
}

impl Default for ChangeCategory {
    fn default() -> Self {
        Self::Other
    }
}

impl std::fmt::Display for ChangeCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Feature => write!(f, "feature"),
            Self::Bugfix => write!(f, "bugfix"),
            Self::Refactor => write!(f, "refactor"),
            Self::Test => write!(f, "test"),
            Self::Documentation => write!(f, "documentation"),
            Self::Configuration => write!(f, "configuration"),
            Self::Dependency => write!(f, "dependency"),
            Self::Formatting => write!(f, "formatting"),
            Self::Other => write!(f, "other"),
        }
    }
}

/// Semantic analysis of a single hunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HunkAnalysis {
    /// The hunk this analysis is for
    pub hunk_id: usize,
    /// Category of the change
    pub category: ChangeCategory,
    /// What this change does semantically
    /// e.g., ["add function validate_token", "import AuthError"]
    pub semantic_units: Vec<String>,
    /// Topic for grouping related changes
    /// e.g., "authentication", "error-handling", "api-client"
    pub topic: String,
    /// Description of what context/dependencies this change needs
    pub depends_on_context: Option<String>,
    /// File path (for convenience in clustering)
    #[serde(skip_deserializing)]
    pub file_path: String,
}

/// LLM response format for hunk analysis
#[derive(Debug, Clone, Deserialize)]
pub struct HunkAnalysisResponse {
    pub category: ChangeCategory,
    pub semantic_units: Vec<String>,
    pub suggested_topic: String,
    #[serde(default)]
    pub depends_on_context: Option<String>,
}

/// A cluster of hunks that should be in the same commit
#[derive(Debug, Clone)]
pub struct Cluster {
    /// Unique identifier for this cluster
    pub id: ClusterId,
    /// Hunks in this cluster
    pub hunk_ids: Vec<HunkId>,
    /// Primary topic of this cluster
    pub topic: String,
    /// Categories present in this cluster
    pub categories: HashSet<ChangeCategory>,
    /// Reason this cluster was formed
    pub formation_reason: ClusterFormationReason,
}

/// Unique identifier for a cluster
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClusterId(pub usize);

impl std::fmt::Display for ClusterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cluster#{}", self.0)
    }
}

/// Why a cluster was formed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClusterFormationReason {
    /// Hunks are in the same file
    SameFile(String),
    /// Hunks share the same topic
    SameTopic(String),
    /// Hunks have dependencies on each other
    Dependencies,
    /// LLM determined these should be together
    LlmGrouped(String),
    /// Manual/fallback grouping
    Fallback,
}

impl std::fmt::Display for ClusterFormationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SameFile(path) => write!(f, "same file: {}", path),
            Self::SameTopic(topic) => write!(f, "same topic: {}", topic),
            Self::Dependencies => write!(f, "dependencies"),
            Self::LlmGrouped(reason) => write!(f, "LLM grouped: {}", reason),
            Self::Fallback => write!(f, "fallback"),
        }
    }
}

/// A planned commit from clustering
#[derive(Debug, Clone)]
pub struct ClusterCommit {
    /// The cluster this commit came from
    pub cluster_id: ClusterId,
    /// Short commit message
    pub short_message: String,
    /// Long commit message
    pub long_message: String,
    /// Hunks in this commit, in order
    pub hunk_ids: Vec<HunkId>,
    /// Other clusters this depends on (must be committed first)
    pub depends_on: Vec<ClusterId>,
}

/// LLM response for commit planning
#[derive(Debug, Clone, Deserialize)]
pub struct CommitPlanResponse {
    pub short_message: String,
    pub long_message: String,
    /// Whether to split this cluster into multiple commits
    #[serde(default)]
    pub should_split: bool,
    /// If splitting, how to split
    #[serde(default)]
    pub split_groups: Option<Vec<SplitGroup>>,
}

/// A group of hunks when splitting a cluster
#[derive(Debug, Clone, Deserialize)]
pub struct SplitGroup {
    pub hunk_ids: Vec<usize>,
    pub short_message: String,
    pub long_message: String,
}

/// LLM response for cross-file relationship detection
#[derive(Debug, Clone, Deserialize)]
pub struct RelationshipResponse {
    pub groups: Vec<RelatedGroup>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelatedGroup {
    pub hunk_ids: Vec<usize>,
    pub reason: String,
}

/// Analysis results for all hunks
#[derive(Debug, Clone, Default)]
pub struct AnalysisResults {
    /// Per-hunk analysis
    pub analyses: HashMap<HunkId, HunkAnalysis>,
    /// Hunks grouped by topic
    pub by_topic: HashMap<String, Vec<HunkId>>,
    /// Hunks grouped by category
    pub by_category: HashMap<ChangeCategory, Vec<HunkId>>,
    /// Hunks grouped by file path
    pub by_file: HashMap<String, Vec<HunkId>>,
}

impl AnalysisResults {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, analysis: HunkAnalysis) {
        let hunk_id = HunkId(analysis.hunk_id);

        // Index by topic
        self.by_topic
            .entry(analysis.topic.clone())
            .or_default()
            .push(hunk_id);

        // Index by category
        self.by_category
            .entry(analysis.category)
            .or_default()
            .push(hunk_id);

        // Index by file
        self.by_file
            .entry(analysis.file_path.clone())
            .or_default()
            .push(hunk_id);

        // Store the analysis
        self.analyses.insert(hunk_id, analysis);
    }

    pub fn get(&self, hunk_id: HunkId) -> Option<&HunkAnalysis> {
        self.analyses.get(&hunk_id)
    }

    pub fn topics(&self) -> impl Iterator<Item = &String> {
        self.by_topic.keys()
    }

    pub fn hunks_for_topic(&self, topic: &str) -> &[HunkId] {
        self.by_topic.get(topic).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Errors specific to hierarchical reorganization
#[derive(Debug, thiserror::Error)]
pub enum HierarchicalError {
    #[error("Analysis failed for hunk {0}: {1}")]
    AnalysisFailed(usize, String),

    #[error("Clustering failed: {0}")]
    ClusteringFailed(String),

    #[error("Commit planning failed for cluster {0}: {1}")]
    PlanningFailed(ClusterId, String),

    #[error("Validation failed: {0}")]
    ValidationFailed(String),

    #[error("LLM error: {0}")]
    LlmError(String),

    #[error("All hunks must be assigned to clusters")]
    UnassignedHunks(Vec<HunkId>),

    #[error("Cyclic dependency detected in clusters")]
    CyclicDependency,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_change_category_display() {
        assert_eq!(ChangeCategory::Feature.to_string(), "feature");
        assert_eq!(ChangeCategory::Bugfix.to_string(), "bugfix");
    }

    #[test]
    fn test_analysis_results_indexing() {
        let mut results = AnalysisResults::new();

        results.add(HunkAnalysis {
            hunk_id: 0,
            category: ChangeCategory::Feature,
            semantic_units: vec!["add function".to_string()],
            topic: "auth".to_string(),
            depends_on_context: None,
            file_path: "src/auth.rs".to_string(),
        });

        results.add(HunkAnalysis {
            hunk_id: 1,
            category: ChangeCategory::Feature,
            semantic_units: vec!["add route".to_string()],
            topic: "auth".to_string(),
            depends_on_context: None,
            file_path: "src/routes.rs".to_string(),
        });

        results.add(HunkAnalysis {
            hunk_id: 2,
            category: ChangeCategory::Test,
            semantic_units: vec!["add test".to_string()],
            topic: "auth".to_string(),
            depends_on_context: None,
            file_path: "tests/auth_test.rs".to_string(),
        });

        // Check topic grouping
        let auth_hunks = results.hunks_for_topic("auth");
        assert_eq!(auth_hunks.len(), 3);

        // Check category grouping
        assert_eq!(results.by_category.get(&ChangeCategory::Feature).unwrap().len(), 2);
        assert_eq!(results.by_category.get(&ChangeCategory::Test).unwrap().len(), 1);

        // Check file grouping
        assert_eq!(results.by_file.len(), 3);
    }

    #[test]
    fn test_cluster_formation_reason_display() {
        let reason = ClusterFormationReason::SameTopic("authentication".to_string());
        assert!(reason.to_string().contains("authentication"));
    }
}
