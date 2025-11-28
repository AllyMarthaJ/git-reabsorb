mod by_file;
pub mod hierarchical;
pub mod llm;
pub mod plan_file;
mod preserve;
mod squash;

pub use by_file::GroupByFile;
pub use hierarchical::{HierarchicalConfig, HierarchicalReorganizer};
pub use llm::LlmReorganizer;
pub use plan_file::{delete_plan, has_saved_plan, load_plan, save_plan, PlanFileError, SavedPlan};
pub use preserve::PreserveOriginal;
pub use squash::Squash;

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

/// Trait for reorganizing hunks into planned commits
pub trait Reorganizer {
    /// Take source commits and hunks, return a plan for new commits
    fn reorganize(
        &self,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError>;

    /// Human-readable name for this strategy
    fn name(&self) -> &'static str;
}
