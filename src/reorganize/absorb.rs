//! Wrapper strategy for git-absorb.

use std::process::Command;

use log::info;

use crate::git::GitOps;
use crate::models::{Hunk, PlannedCommit, SourceCommit};
use crate::reorganize::{ApplyResult, ReorganizeError, Reorganizer};

/// Wraps git-absorb as a reorganization strategy.
pub struct Absorb;

impl Reorganizer for Absorb {
    fn plan(
        &self,
        _source_commits: &[SourceCommit],
        _hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        if !crate::features::Features::global().is_enabled(crate::features::Feature::Absorb) {
            return Err(ReorganizeError::Failed(
                "The 'absorb' feature is not enabled. Enable it with --features=gitabsorb or the GIT_REABSORB_FEATURES environment variable.".to_string()
            ));
        }
        // git-absorb handles everything in apply, nothing to plan
        Ok(Vec::new())
    }

    fn apply(
        &self,
        _git: &dyn GitOps,
        extra_args: &[String],
    ) -> Result<ApplyResult, ReorganizeError> {
        info!("Running git-absorb...");

        let mut cmd = Command::new("git-absorb");
        cmd.args(extra_args);

        let output = cmd
            .output()
            .map_err(|e| ReorganizeError::Failed(format!("Failed to run git-absorb: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ReorganizeError::Failed(format!(
                "git-absorb failed: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.is_empty() {
            for line in stdout.lines() {
                info!("{}", line);
            }
        }

        Ok(ApplyResult::Handled)
    }

    fn name(&self) -> &'static str {
        "absorb"
    }
}
