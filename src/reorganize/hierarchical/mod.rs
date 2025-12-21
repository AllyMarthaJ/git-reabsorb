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
//! - Debuggable (each phase produces inspectable intermediate results)

mod analyzer;
mod clusterer;
mod orderer;
mod planner;
mod types;
mod validator;

pub use analyzer::HunkAnalyzer;
pub use clusterer::{ClusterConfig, Clusterer};
pub use orderer::GlobalOrderer;
pub use planner::CommitPlanner;
pub use types::*;
pub use validator::{assign_orphans, deduplicate_across_commits, to_planned_commits, Validator};

use std::sync::Arc;

use log::{debug, info};

use crate::features::Feature;
use crate::llm::LlmClient;
use crate::models::{Hunk, PlannedCommit, SourceCommit};
use crate::reorganize::{ReorganizeError, Reorganizer};

/// Configuration for the hierarchical reorganizer
#[derive(Debug, Clone)]
pub struct HierarchicalConfig {
    /// Maximum parallel LLM calls
    pub max_parallel: usize,
    /// Cluster configuration
    pub cluster_config: ClusterConfig,
}

impl Default for HierarchicalConfig {
    fn default() -> Self {
        Self {
            max_parallel: 8,
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
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        let client = self.client.as_ref().ok_or_else(|| {
            ReorganizeError::InvalidPlan(
                "LLM client is required for hierarchical reorganization".to_string(),
            )
        })?;

        info!("Phase 1: Analyzing {} hunks...", hunks.len());

        // Phase 1: Analyze hunks
        let analyzer =
            HunkAnalyzer::new(Arc::clone(client)).with_parallelism(self.config.max_parallel);

        let analysis = analyzer.analyze(hunks, source_commits)?;

        debug!(
            "  Found {} topics: {:?}",
            analysis.by_topic.len(),
            analysis.topics().take(5).collect::<Vec<_>>()
        );

        info!("Phase 2: Clustering hunks...");

        // Phase 2: Cluster hunks
        let clusterer = Clusterer::new(Some(Arc::clone(client)))
            .with_config(self.config.cluster_config.clone());

        let clusters = clusterer.cluster(hunks, &analysis)?;

        debug!("  Created {} clusters", clusters.len());

        info!("Phase 3: Planning commits...");

        // Phase 3: Plan commits
        let planner =
            CommitPlanner::new(Some(Arc::clone(client))).with_parallelism(self.config.max_parallel);

        let commits = planner.plan(&clusters, hunks, &analysis)?;

        debug!("  Planned {} commits", commits.len());

        info!("Phase 4: Ordering commits...");

        // Phase 4: Order commits
        let ordered = GlobalOrderer::order(commits, &analysis)?;

        info!("Phase 5: Validating and repairing...");

        // Phase 5: Validate and repair
        let validator = Validator::new(self.client.clone());
        let validations = validator.validate(&ordered, hunks);

        let invalid_count = validations.iter().filter(|v| !v.is_valid).count();
        if invalid_count > 0 {
            debug!("  Found {} invalid commits, repairing...", invalid_count);
        }

        let repaired = validator
            .repair(ordered, &validations, hunks, &analysis)
            .map_err(|e| ReorganizeError::InvalidPlan(e.to_string()))?;

        // Apply cross-commit fixes under feature flag
        let final_commits = if Feature::AttemptValidationFix.is_enabled() {
            // Deduplicate hunks assigned to multiple commits
            let deduped = deduplicate_across_commits(repaired);
            // Assign any orphaned hunks
            assign_orphans(deduped, hunks, &analysis)
        } else {
            repaired
        };

        // Final validation
        validator
            .validate_complete_assignment(&final_commits, hunks)
            .map_err(|e| ReorganizeError::InvalidPlan(e.to_string()))?;

        debug!("  Final: {} commits", final_commits.len());

        // Convert to PlannedCommits
        Ok(to_planned_commits(final_commits))
    }
}

impl Reorganizer for HierarchicalReorganizer {
    fn plan(
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
    use crate::models::DiffLine;
    use crate::test_utils::{make_hunk_full, make_source_commit};

    #[test]
    fn test_empty_hunks() {
        let reorganizer = HierarchicalReorganizer::new(None);
        let result = reorganizer.plan(&[], &[]);

        assert!(matches!(result, Err(ReorganizeError::NoHunks)));
    }

    #[test]
    fn test_requires_llm_client() {
        let hunks = vec![make_hunk_full(
            0,
            "src/main.rs",
            vec![DiffLine::Added("fn main() {}".to_string())],
            vec!["abc123".to_string()],
        )];

        let source_commits = vec![make_source_commit("abc123", "Add main")];

        let reorganizer = HierarchicalReorganizer::new(None);
        let result = reorganizer.plan(&source_commits, &hunks);

        // Should error without an LLM client
        assert!(matches!(result, Err(ReorganizeError::InvalidPlan(_))));
    }
}
