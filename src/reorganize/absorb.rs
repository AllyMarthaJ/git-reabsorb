//! Absorb as a reorganization strategy: the working tree's changes fold into
//! the branch commits that last touched their lines, and the rest stays
//! uncommitted.

use log::info;

use crate::absorb::{absorb, Leftover};
use crate::git::GitOps;
use crate::models::{Hunk, PlannedCommit, SourceCommit};
use crate::reorganize::{ApplyResult, ReorganizeError, Reorganizer};

pub struct Absorb;

impl Reorganizer for Absorb {
    fn plan(
        &self,
        _source_commits: &[SourceCommit],
        _hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        // Absorbing decides everything in apply, against the branch as it is then.
        Ok(Vec::new())
    }

    fn apply(&self, git: &dyn GitOps, base: &str) -> Result<ApplyResult, ReorganizeError> {
        let absorption = absorb(git, base, &Leftover::Keep)
            .map_err(|error| ReorganizeError::Failed(error.to_string()))?;
        for (commit, hunks) in &absorption.absorbed {
            info!("Absorbed {hunks} hunk(s) into {commit}");
        }
        Ok(ApplyResult::Handled)
    }

    fn name(&self) -> &'static str {
        "absorb"
    }
}
