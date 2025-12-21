mod absorb;
mod by_file;
pub mod hierarchical;
pub mod llm;
mod preserve;
mod squash;

pub use absorb::Absorb;
pub use by_file::GroupByFile;
pub use hierarchical::{HierarchicalConfig, HierarchicalReorganizer};
pub use llm::LlmReorganizer;
pub use preserve::PreserveOriginal;
pub use squash::Squash;

use crate::git::GitOps;
use crate::models::{Hunk, PlannedCommit, SourceCommit};
use crate::validation::ValidationResult;

/// Errors from reorganization
#[derive(Debug, thiserror::Error)]
pub enum ReorganizeError {
    #[error("No hunks to reorganize")]
    NoHunks,
    #[error("Reorganization failed: {0}")]
    Failed(String),
    #[error("Invalid plan: {0}")]
    InvalidPlan(String),
}

impl From<hierarchical::HierarchicalError> for ReorganizeError {
    fn from(err: hierarchical::HierarchicalError) -> Self {
        ReorganizeError::Failed(err.to_string())
    }
}

/// Result of apply indicating whether to continue with default execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyResult {
    /// Strategy handled everything, skip default execution
    Handled,
    /// Continue with default hunk-based execution
    Continue,
}

/// Trait for reorganizing hunks into planned commits
pub trait Reorganizer {
    /// Take source commits and hunks, return a plan for new commits
    fn plan(
        &self,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError>;

    /// Attempt to fix validation issues in a plan.
    ///
    /// The default implementation simply retries by calling `plan` again.
    /// Strategies can override this to provide smarter fixes (e.g., targeted
    /// LLM prompts for specific issues).
    ///
    /// # Arguments
    /// * `commits` - The current (invalid) plan
    /// * `validation` - The validation result with issues to fix
    /// * `source_commits` - Original source commits
    /// * `hunks` - All hunks being reorganized
    fn fix_plan(
        &self,
        _commits: Vec<PlannedCommit>,
        _validation: &ValidationResult,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        // Default: retry the plan from scratch
        self.plan(source_commits, hunks)
    }

    /// Apply the strategy. Returns whether to continue with default execution.
    fn apply(
        &self,
        _git: &dyn GitOps,
        _extra_args: &[String],
    ) -> Result<ApplyResult, ReorganizeError> {
        Ok(ApplyResult::Continue)
    }

    /// Human-readable name for this strategy
    fn name(&self) -> &'static str;
}
