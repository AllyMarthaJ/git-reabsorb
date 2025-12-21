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
