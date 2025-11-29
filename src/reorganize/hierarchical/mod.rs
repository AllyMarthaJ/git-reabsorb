//! Hierarchical multi-phase reorganization strategy
//!
//! This module implements a robust commit reorganization strategy that scales
//! to many thousands of lines of changes by breaking the problem into phases:
//!
//! 1. **Analysis**: Each hunk is analyzed independently (in parallel) to extract
//!    semantic metadata like category, topic, and dependencies.
//!
//! 2. **Clustering**: Hunks are grouped into candidate commits based on topic,
//!    file relationships, and cross-file dependencies detected by LLM.
//!
//! 3. **Planning**: Each cluster gets a commit message generated (in parallel),
//!    with the option to split clusters that contain unrelated changes.
//!
//! 4. **Ordering**: Commits are ordered based on dependencies and category
//!    (dependencies before features before tests, etc.).
//!
//! 5. **Validation**: Commits are validated and repaired if needed, ensuring
//!    all hunks are assigned exactly once.
//!
//! ## Advantages over single-shot LLM approach
//!
//! - Scales to thousands of hunks (each LLM call is small and focused)
//! - Parallelizable (analysis and planning phases run concurrently)
//! - Incremental repair (fix individual commits without redoing everything)
//! - Deterministic fallbacks (heuristics work when LLM is unavailable)
//! - Debuggable (each phase produces inspectable intermediate results)

mod analyzer;
mod clusterer;
mod orderer;
mod planner;
mod types;
mod validator;

pub use analyzer::{HeuristicAnalyzer, HunkAnalyzer};
pub use clusterer::{ClusterConfig, Clusterer, HeuristicClusterer};
pub use orderer::{GlobalOrderer, HeuristicOrderer};
pub use planner::{CommitPlanner, HeuristicPlanner};
pub use types::*;
pub use validator::{assign_orphans, to_planned_commits, Validator};

use std::sync::Arc;

use crate::models::{Hunk, PlannedCommit, SourceCommit};
use crate::reorganize::llm::LlmClient;
use crate::reorganize::{ReorganizeError, Reorganizer};

/// Configuration for the hierarchical reorganizer
#[derive(Debug, Clone)]
pub struct HierarchicalConfig {
    /// Whether to use LLM for analysis (false = heuristics only)
    pub use_llm_analysis: bool,
    /// Whether to use LLM for clustering refinement
    pub use_llm_clustering: bool,
    /// Whether to use LLM for commit message generation
    pub use_llm_planning: bool,
    /// Maximum parallel LLM calls
    pub max_parallel: usize,
    /// Cluster configuration
    pub cluster_config: ClusterConfig,
}

impl Default for HierarchicalConfig {
    fn default() -> Self {
        Self {
            use_llm_analysis: true,
            use_llm_clustering: true,
            use_llm_planning: true,
            max_parallel: 8,
            cluster_config: ClusterConfig::default(),
        }
    }
}

impl HierarchicalConfig {
    /// Create a config that uses only heuristics (no LLM)
    pub fn heuristic_only() -> Self {
        Self {
            use_llm_analysis: false,
            use_llm_clustering: false,
            use_llm_planning: false,
            max_parallel: 1,
            cluster_config: ClusterConfig::default(),
        }
    }
}

/// Multi-phase hierarchical reorganizer
pub struct HierarchicalReorganizer {
    client: Option<Arc<dyn LlmClient + Send + Sync>>,
    config: HierarchicalConfig,
}

impl HierarchicalReorganizer {
    pub fn new(client: Option<Arc<dyn LlmClient + Send + Sync>>) -> Self {
        Self {
            client,
            config: HierarchicalConfig::default(),
        }
    }

    pub fn with_config(mut self, config: HierarchicalConfig) -> Self {
        self.config = config;
        self
    }

    /// Run the full reorganization pipeline
    fn run_pipeline(
        &self,
        _source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        eprintln!("Phase 1: Analyzing {} hunks...", hunks.len());

        // Phase 1: Analyze hunks
        let analysis = if self.config.use_llm_analysis {
            if let Some(ref client) = self.client {
                let analyzer = HunkAnalyzer::new(Arc::clone(client))
                    .with_parallelism(self.config.max_parallel);

                analyzer
                    .analyze(hunks)
                    .map_err(|e| {
                        eprintln!("LLM analysis failed, falling back to heuristics: {}", e);
                        e
                    })
                    .unwrap_or_else(|_| HeuristicAnalyzer::analyze(hunks))
            } else {
                HeuristicAnalyzer::analyze(hunks)
            }
        } else {
            HeuristicAnalyzer::analyze(hunks)
        };

        eprintln!(
            "  Found {} topics: {:?}",
            analysis.by_topic.len(),
            analysis.topics().take(5).collect::<Vec<_>>()
        );

        eprintln!("Phase 2: Clustering hunks...");

        // Phase 2: Cluster hunks
        let cluster_client = if self.config.use_llm_clustering {
            self.client.clone()
        } else {
            None
        };

        let clusters = if cluster_client.is_some() {
            let clusterer =
                Clusterer::new(cluster_client).with_config(self.config.cluster_config.clone());

            clusterer
                .cluster(hunks, &analysis)
                .map_err(|e| {
                    eprintln!("LLM clustering failed, falling back to heuristics: {}", e);
                    e
                })
                .unwrap_or_else(|_| HeuristicClusterer::cluster(hunks, &analysis))
        } else {
            HeuristicClusterer::cluster(hunks, &analysis)
        };

        eprintln!("  Created {} clusters", clusters.len());

        eprintln!("Phase 3: Planning commits...");

        // Phase 3: Plan commits
        let plan_client = if self.config.use_llm_planning {
            self.client.clone()
        } else {
            None
        };

        let commits = if plan_client.is_some() {
            let planner =
                CommitPlanner::new(plan_client).with_parallelism(self.config.max_parallel);

            planner
                .plan(&clusters, hunks, &analysis)
                .map_err(|e| {
                    eprintln!("LLM planning failed, falling back to heuristics: {}", e);
                    e
                })
                .unwrap_or_else(|_| HeuristicPlanner::plan(&clusters, &analysis))
        } else {
            HeuristicPlanner::plan(&clusters, &analysis)
        };

        eprintln!("  Planned {} commits", commits.len());

        eprintln!("Phase 4: Ordering commits...");

        // Phase 4: Order commits
        let ordered = GlobalOrderer::order(commits, &analysis).unwrap_or_else(|e| {
            eprintln!("Ordering failed ({}), using heuristic order", e);
            // Recover: use heuristic ordering on the original commits
            HeuristicOrderer::order(HeuristicPlanner::plan(&clusters, &analysis), &analysis)
        });

        eprintln!("Phase 5: Validating and repairing...");

        // Phase 5: Validate and repair
        let validator = Validator::new(self.client.clone());
        let validations = validator.validate(&ordered, hunks);

        let invalid_count = validations.iter().filter(|v| !v.is_valid).count();
        if invalid_count > 0 {
            eprintln!("  Found {} invalid commits, repairing...", invalid_count);
        }

        let repaired = validator
            .repair(ordered, &validations, hunks, &analysis)
            .map_err(|e| ReorganizeError::InvalidPlan(e.to_string()))?;

        // Assign any orphaned hunks
        let final_commits = assign_orphans(repaired, hunks, &analysis);

        // Final validation
        validator
            .validate_complete_assignment(&final_commits, hunks)
            .map_err(|e| ReorganizeError::InvalidPlan(e.to_string()))?;

        eprintln!("  Final: {} commits", final_commits.len());

        // Convert to PlannedCommits
        Ok(to_planned_commits(final_commits))
    }
}

impl Reorganizer for HierarchicalReorganizer {
    fn reorganize(
        &self,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        if hunks.is_empty() {
            return Err(ReorganizeError::NoHunks);
        }

        self.run_pipeline(source_commits, hunks)
    }

    fn name(&self) -> &'static str {
        "hierarchical"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DiffLine, HunkId};
    use std::path::PathBuf;

    fn make_test_hunk(id: usize, file: &str, lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            id: HunkId(id),
            file_path: PathBuf::from(file),
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 4,
            lines,
            likely_source_commits: vec!["abc123".to_string()],
        }
    }

    fn make_source_commit(sha: &str, message: &str) -> SourceCommit {
        SourceCommit {
            sha: sha.to_string(),
            short_description: message.to_string(),
            long_description: message.to_string(),
        }
    }

    #[test]
    fn test_heuristic_only_reorganization() {
        let hunks = vec![
            make_test_hunk(
                0,
                "src/auth/login.rs",
                vec![DiffLine::Added("pub fn login() {}".to_string())],
            ),
            make_test_hunk(
                1,
                "src/auth/logout.rs",
                vec![DiffLine::Added("pub fn logout() {}".to_string())],
            ),
            make_test_hunk(
                2,
                "tests/auth_test.rs",
                vec![DiffLine::Added("#[test] fn test_auth() {}".to_string())],
            ),
        ];

        let source_commits = vec![make_source_commit("abc123", "Initial implementation")];

        let config = HierarchicalConfig::heuristic_only();
        let reorganizer = HierarchicalReorganizer::new(None).with_config(config);

        let result = reorganizer.reorganize(&source_commits, &hunks);

        assert!(result.is_ok());
        let commits = result.unwrap();

        // Should have at least one commit
        assert!(!commits.is_empty());

        // All hunks should be assigned
        let total_hunks: usize = commits.iter().map(|c| c.changes.len()).sum();
        assert_eq!(total_hunks, 3);
    }

    #[test]
    fn test_empty_hunks() {
        let reorganizer = HierarchicalReorganizer::new(None);
        let result = reorganizer.reorganize(&[], &[]);

        assert!(matches!(result, Err(ReorganizeError::NoHunks)));
    }

    #[test]
    fn test_single_hunk() {
        let hunks = vec![make_test_hunk(
            0,
            "src/main.rs",
            vec![DiffLine::Added("fn main() {}".to_string())],
        )];

        let source_commits = vec![make_source_commit("abc123", "Add main")];

        let config = HierarchicalConfig::heuristic_only();
        let reorganizer = HierarchicalReorganizer::new(None).with_config(config);

        let result = reorganizer.reorganize(&source_commits, &hunks);

        assert!(result.is_ok());
        let commits = result.unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].changes.len(), 1);
    }

    #[test]
    fn test_multi_file_same_topic() {
        // Changes across multiple files that should be grouped together
        let hunks = vec![
            make_test_hunk(
                0,
                "src/api/users.rs",
                vec![DiffLine::Added("pub fn get_user() {}".to_string())],
            ),
            make_test_hunk(
                1,
                "src/api/users.rs",
                vec![DiffLine::Added("pub fn create_user() {}".to_string())],
            ),
            make_test_hunk(
                2,
                "src/api/routes.rs",
                vec![DiffLine::Added("route(\"/users\")".to_string())],
            ),
        ];

        let source_commits = vec![make_source_commit("abc123", "Add user API")];

        let config = HierarchicalConfig::heuristic_only();
        let reorganizer = HierarchicalReorganizer::new(None).with_config(config);

        let result = reorganizer.reorganize(&source_commits, &hunks);

        assert!(result.is_ok());
        let commits = result.unwrap();

        // All hunks should be in commits
        let total_hunks: usize = commits.iter().map(|c| c.changes.len()).sum();
        assert_eq!(total_hunks, 3);
    }
}
