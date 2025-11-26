mod by_file;
mod preserve;
mod squash;

pub use by_file::GroupByFile;
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
