//! LLM-based reorganization strategy
//!
//! This module contains reorganization-specific LLM code:
//! - Prompt construction for commit planning
//! - Response parsing and validation
//! - The `LlmReorganizer` strategy
//!
//! Generic LLM infrastructure lives in `crate::llm`.

mod file_io;
mod parser;
mod prompt;
mod types;

pub use prompt::build_reword_prompt;
pub use types::{ChangeSpec, FixMessageResponse};

use log::{debug, info};

use crate::features::Feature;
use crate::llm::{LlmClient, LlmError};
use crate::models::{
    CommitDescription, Hunk, HunkId, PlannedChange, PlannedCommit, PlannedCommitId,
    SourceCommit,
};
use crate::reorganize::{ReorganizeError, Reorganizer};
use crate::utils::extract_json_str;
use crate::validation::{ValidationIssue, ValidationResult};

use types::{FixDuplicateResponse, FixOverlappingResponse, FixUnassignedResponse, HunkAssignment};

pub struct LlmReorganizer {
    client: Box<dyn LlmClient>,
    max_retries: usize,
}

impl LlmReorganizer {
    pub fn new(client: Box<dyn LlmClient>) -> Self {
        Self {
            client,
            max_retries: 3,
        }
    }

    pub fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Invoke LLM with retry for parse errors only
    fn invoke_with_retry(
        &self,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, LlmError> {
        let context = prompt::build_context(source_commits, hunks);

        // Set up file-based I/O if feature is enabled
        let (prompt_text, _file_session, use_file_io) = if Feature::FileBasedLlmIo.is_enabled() {
            let session = file_io::LlmFileSession::new()?;

            // Write hunks to input file
            let hunks_content = prompt::build_hunks_file_content(&context);
            session.write_input(&hunks_content)?;

            debug!(
                "File-based LLM I/O: input={}",
                session.input_path.display()
            );

            // Build prompt that references the input file
            let prompt = prompt::build_file_based_prompt(&context, &session.input_path);

            (prompt, Some(session), true)
        } else {
            (prompt::build_prompt(&context), None, false)
        };

        let mut last_error = None;

        for attempt in 1..=self.max_retries {
            info!("LLM attempt {}/{}...", attempt, self.max_retries);
            match self.client.complete(&prompt_text) {
                Ok(stdout_response) => {
                    // Get response from file (via path in stdout) or directly from stdout
                    let response = if use_file_io {
                        match file_io::read_response_from_path(&stdout_response) {
                            Ok(file_content) => {
                                debug!("Read response from file path in stdout");
                                file_content
                            }
                            Err(e) => {
                                debug!("File read failed ({}), cannot proceed", e);
                                last_error = Some(e);
                                continue;
                            }
                        }
                    } else {
                        stdout_response
                    };

                    match parser::extract_json(&response) {
                        Ok(llm_commits) => {
                            // Convert to PlannedCommits immediately
                            match parser::to_planned_commits(llm_commits, hunks) {
                                Ok(commits) => return Ok(commits),
                                Err(e) => {
                                    debug!("Conversion error: {}", e);
                                    last_error = Some(e);
                                }
                            }
                        }
                        Err(e) => {
                            debug!("Parse error: {}", e);
                            last_error = Some(e);
                        }
                    }
                }
                Err(e) => {
                    debug!("Client error: {}", e);
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.unwrap_or(LlmError::MaxRetriesExceeded(self.max_retries)))
    }

    /// Apply unassigned hunk fixes directly to PlannedCommits
    fn apply_unassigned_fix_to_commits(
        &self,
        commits: &mut Vec<PlannedCommit>,
        fix: FixUnassignedResponse,
    ) {
        for assignment in fix.assignments {
            match assignment {
                HunkAssignment::AddToExisting {
                    hunk_id,
                    commit_description,
                } => {
                    // Find the commit by description and add the hunk
                    if let Some(commit) = commits
                        .iter_mut()
                        .find(|c| c.description.short == commit_description)
                    {
                        commit.changes.push(PlannedChange::ExistingHunk(HunkId(hunk_id)));
                        debug!(
                            "  Added hunk {} to commit '{}'",
                            hunk_id, commit_description
                        );
                    }
                }
                HunkAssignment::NewCommit {
                    hunk_id,
                    short_description,
                    long_description,
                } => {
                    // Create a new commit
                    let next_id = commits.iter().map(|c| c.id.0).max().unwrap_or(0) + 1;
                    commits.push(PlannedCommit::new(
                        PlannedCommitId(next_id),
                        CommitDescription::new(&short_description, &long_description),
                        vec![PlannedChange::ExistingHunk(HunkId(hunk_id))],
                    ));
                    debug!(
                        "  Created new commit '{}' for hunk {}",
                        short_description, hunk_id
                    );
                }
            }
        }
    }
}

impl Reorganizer for LlmReorganizer {
    fn plan(
        &self,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        if hunks.is_empty() {
            return Err(ReorganizeError::NoHunks);
        }
        self.invoke_with_retry(source_commits, hunks)
            .map_err(|e| ReorganizeError::InvalidPlan(e.to_string()))
    }

    fn fix_plan(
        &self,
        mut commits: Vec<PlannedCommit>,
        validation: &ValidationResult,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        if !Feature::AttemptValidationFix.is_enabled() {
            // Retry from scratch
            debug!("Retrying LLM plan from scratch...");
            return self.plan(source_commits, hunks);
        }

        debug!("Applying LLM-based fixes to plan...");

        // Build context for prompts
        let context = prompt::build_context(source_commits, hunks);

        // Fix duplicate hunks using LLM
        for (hunk_id, commit_ids) in validation.duplicate_hunks() {
            debug!("Fixing duplicate hunk {} across {:?}", hunk_id.0, commit_ids);

            // Convert commit_ids to indices
            let commit_indices: Vec<usize> = commit_ids
                .iter()
                .filter_map(|id| commits.iter().position(|c| c.id == *id))
                .collect();

            if commit_indices.len() < 2 {
                continue; // Not actually a cross-commit duplicate
            }

            let fix_prompt = prompt::build_fix_duplicate_prompt(
                &context,
                &commits,
                hunk_id.0,
                &commit_indices,
            );

            match self.client.complete(&fix_prompt) {
                Ok(response) => {
                    if let Some(json_str) = extract_json_str(&response) {
                        if let Ok(fix) = serde_json::from_str::<FixDuplicateResponse>(json_str) {
                            // Apply fix: remove hunk from all commits except chosen one
                            for (idx, commit) in commits.iter_mut().enumerate() {
                                if idx != fix.chosen_commit_index {
                                    commit.changes.retain(|c| {
                                        !matches!(c, PlannedChange::ExistingHunk(id) if *id == hunk_id)
                                    });
                                }
                            }
                            debug!("  Kept hunk {} in commit index {}", hunk_id.0, fix.chosen_commit_index);
                        }
                    }
                }
                Err(e) => {
                    debug!("  LLM fix failed for duplicate hunk {}: {}", hunk_id.0, e);
                }
            }
        }

        // Fix overlapping hunks using LLM
        for (file_path, hunk_a, commit_a, hunk_b, commit_b) in validation.overlapping_hunks() {
            debug!(
                "Fixing overlapping hunks {} and {} in {} (commits {:?} and {:?})",
                hunk_a.0,
                hunk_b.0,
                file_path.display(),
                commit_a,
                commit_b
            );

            // Convert commit IDs to indices
            let commit_a_idx = commits.iter().position(|c| c.id == commit_a);
            let commit_b_idx = commits.iter().position(|c| c.id == commit_b);

            let (Some(commit_a_idx), Some(commit_b_idx)) = (commit_a_idx, commit_b_idx) else {
                debug!("  Could not find commit indices, skipping");
                continue;
            };

            let fix_prompt = prompt::build_fix_overlapping_prompt(
                &context,
                &commits,
                hunk_a.0,
                commit_a_idx,
                hunk_b.0,
                commit_b_idx,
                &file_path,
            );

            match self.client.complete(&fix_prompt) {
                Ok(response) => {
                    if let Some(json_str) = extract_json_str(&response) {
                        if let Ok(fix) = serde_json::from_str::<FixOverlappingResponse>(json_str) {
                            // Move both hunks to the chosen commit
                            let (keep_idx, move_idx, move_hunk) =
                                if fix.chosen_commit_index == commit_a_idx {
                                    (commit_a_idx, commit_b_idx, hunk_b)
                                } else {
                                    (commit_b_idx, commit_a_idx, hunk_a)
                                };

                            // Remove the hunk from the non-chosen commit
                            if let Some(commit) = commits.get_mut(move_idx) {
                                commit.changes.retain(|c| {
                                    !matches!(c, PlannedChange::ExistingHunk(id) if *id == move_hunk)
                                });
                            }

                            // Add the hunk to the chosen commit if not already there
                            if let Some(commit) = commits.get_mut(keep_idx) {
                                let already_has =
                                    commit.changes.iter().any(|c| {
                                        matches!(c, PlannedChange::ExistingHunk(id) if *id == move_hunk)
                                    });
                                if !already_has {
                                    commit
                                        .changes
                                        .push(PlannedChange::ExistingHunk(move_hunk));
                                }
                            }

                            debug!(
                                "  Moved hunk {} to commit index {} (with hunk {})",
                                move_hunk.0, keep_idx, if move_hunk == hunk_b { hunk_a.0 } else { hunk_b.0 }
                            );
                        }
                    }
                }
                Err(e) => {
                    debug!(
                        "  LLM fix failed for overlapping hunks {} and {}: {}",
                        hunk_a.0, hunk_b.0, e
                    );
                }
            }
        }

        // Fix unassigned hunks using LLM
        if let Some(unassigned) = validation.unassigned_hunks() {
            if !unassigned.is_empty() {
                debug!("Fixing {} unassigned hunks", unassigned.len());

                let unassigned_ids: Vec<usize> = unassigned.iter().map(|h| h.0).collect();

                let fix_prompt = prompt::build_fix_unassigned_prompt(
                    &context,
                    &commits,
                    &unassigned_ids,
                );

                match self.client.complete(&fix_prompt) {
                    Ok(response) => {
                        if let Some(json_str) = extract_json_str(&response) {
                            if let Ok(fix) = serde_json::from_str::<FixUnassignedResponse>(json_str) {
                                self.apply_unassigned_fix_to_commits(&mut commits, fix);
                            }
                        }
                    }
                    Err(e) => {
                        debug!("  LLM fix failed for unassigned hunks: {}", e);
                    }
                }
            }
        }

        // Fix failed assessment issues (rewrite commit messages)
        for issue in validation.failed_assessments() {
            if let ValidationIssue::FailedAssessment {
                commit_id,
                assessment,
            } = issue
            {
                debug!(
                    "Fixing failed assessment for commit {} (score: {:.1}%)",
                    commit_id,
                    assessment.overall_score * 100.0
                );

                // Find the commit to fix
                if let Some(commit) = commits.iter_mut().find(|c| c.id == *commit_id) {
                    let fix_prompt = prompt::build_fix_message_prompt(commit, assessment, hunks);

                    match self.client.complete(&fix_prompt) {
                        Ok(response) => {
                            if let Some(json_str) = extract_json_str(&response) {
                                if let Ok(fix) = serde_json::from_str::<FixMessageResponse>(json_str)
                                {
                                    debug!(
                                        "  Rewrote commit message: '{}' -> '{}'",
                                        commit.description.short, fix.description.short
                                    );
                                    commit.description = fix.description;
                                }
                            }
                        }
                        Err(e) => {
                            debug!("  LLM fix failed for commit {}: {}", commit_id, e);
                        }
                    }
                }
            }
        }

        // Remove any commits that ended up empty
        commits.retain(|c| !c.changes.is_empty());

        Ok(commits)
    }

    fn name(&self) -> &'static str {
        "llm"
    }
}
