//! LLM-based reorganization strategy
//!
//! This module contains reorganization-specific LLM code:
//! - Prompt construction for commit planning
//! - Response parsing and validation
//! - The `LlmReorganizer` strategy
//!
//! Generic LLM infrastructure lives in `crate::llm`.

mod parser;
mod prompt;
mod types;

pub use types::{ChangeSpec, LlmPlan};

use std::path::PathBuf;

use crate::llm::{LlmClient, LlmError};
use crate::models::{
    CommitDescription, DiffLine, Hunk, HunkId, PlannedChange, PlannedCommit, SourceCommit,
};
use crate::reorganize::{ReorganizeError, Reorganizer};
use crate::utils::extract_json_str;

use parser::ValidationIssue;
use types::{FixDuplicateResponse, FixUnassignedResponse, HunkAssignment, LlmCommit, LlmContext};

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

    fn invoke_with_retry(
        &self,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<LlmPlan, LlmError> {
        let context = prompt::build_context(source_commits, hunks);
        let prompt_text = prompt::build_prompt(&context);
        let mut last_error = None;

        for attempt in 1..=self.max_retries {
            eprintln!("LLM attempt {}/{}...", attempt, self.max_retries);
            match self.client.complete(&prompt_text) {
                Ok(response) => match parser::extract_json(&response) {
                    Ok(mut plan) => {
                        match parser::validate_plan_with_issues(&plan, hunks) {
                            Ok(()) => return Ok(plan),
                            Err((err, Some(issue))) => {
                                eprintln!("Validation error: {}", err);
                                // Try smart fix
                                match self.try_fix_issue(&context, &mut plan, issue, hunks) {
                                    Ok(()) => {
                                        // Re-validate after fix
                                        match parser::validate_plan(&plan, hunks) {
                                            Ok(()) => return Ok(plan),
                                            Err(e) => {
                                                eprintln!("Fix didn't resolve all issues: {}", e);
                                                last_error = Some(e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Fix attempt failed: {}", e);
                                        last_error = Some(e);
                                    }
                                }
                            }
                            Err((err, None)) => {
                                eprintln!("Validation error: {}", err);
                                last_error = Some(err);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Parse error: {}", e);
                        last_error = Some(e);
                    }
                },
                Err(e) => {
                    eprintln!("Client error: {}", e);
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.unwrap_or(LlmError::MaxRetriesExceeded(self.max_retries)))
    }

    /// Try to fix a specific validation issue with a targeted prompt
    fn try_fix_issue(
        &self,
        context: &LlmContext,
        plan: &mut LlmPlan,
        issue: ValidationIssue,
        hunks: &[Hunk],
    ) -> Result<(), LlmError> {
        match issue {
            ValidationIssue::UnassignedHunks(unassigned_ids) => {
                eprintln!(
                    "Attempting to fix {} unassigned hunks...",
                    unassigned_ids.len()
                );
                let fix_prompt =
                    prompt::build_fix_unassigned_prompt(context, plan, &unassigned_ids);
                let response = self.client.complete(&fix_prompt)?;

                let json_str = extract_json_str(&response)
                    .ok_or_else(|| LlmError::ParseError("No JSON in fix response".to_string()))?;

                let fix: FixUnassignedResponse = serde_json::from_str(json_str).map_err(|e| {
                    LlmError::ParseError(format!("Failed to parse fix response: {}", e))
                })?;

                self.apply_unassigned_fix(plan, fix, hunks)?;
                Ok(())
            }
            ValidationIssue::DuplicateHunk {
                hunk_id,
                commit_indices,
            } => {
                eprintln!("Attempting to fix duplicate hunk {}...", hunk_id);
                let fix_prompt =
                    prompt::build_fix_duplicate_prompt(context, plan, hunk_id, &commit_indices);
                let response = self.client.complete(&fix_prompt)?;

                let json_str = extract_json_str(&response)
                    .ok_or_else(|| LlmError::ParseError("No JSON in fix response".to_string()))?;

                let fix: FixDuplicateResponse = serde_json::from_str(json_str).map_err(|e| {
                    LlmError::ParseError(format!("Failed to parse fix response: {}", e))
                })?;

                self.apply_duplicate_fix(plan, hunk_id, fix.chosen_commit_index)?;
                Ok(())
            }
        }
    }

    /// Apply a fix for unassigned hunks
    fn apply_unassigned_fix(
        &self,
        plan: &mut LlmPlan,
        fix: FixUnassignedResponse,
        _hunks: &[Hunk],
    ) -> Result<(), LlmError> {
        for assignment in fix.assignments {
            match assignment {
                HunkAssignment::AddToExisting {
                    hunk_id,
                    commit_description,
                } => {
                    // Find the commit by description and add the hunk
                    let commit = plan
                        .commits
                        .iter_mut()
                        .find(|c| c.description.short == commit_description)
                        .ok_or_else(|| {
                            LlmError::ValidationError(format!(
                                "Commit '{}' not found in plan",
                                commit_description
                            ))
                        })?;
                    commit.changes.push(ChangeSpec::Hunk { id: hunk_id });
                    eprintln!(
                        "  Added hunk {} to commit '{}'",
                        hunk_id, commit_description
                    );
                }
                HunkAssignment::NewCommit {
                    hunk_id,
                    short_description,
                    long_description,
                } => {
                    // Create a new commit
                    plan.commits.push(LlmCommit {
                        description: CommitDescription::new(&short_description, &long_description),
                        changes: vec![ChangeSpec::Hunk { id: hunk_id }],
                    });
                    eprintln!(
                        "  Created new commit '{}' for hunk {}",
                        short_description, hunk_id
                    );
                }
            }
        }
        Ok(())
    }

    /// Apply a fix for duplicate hunk assignment
    fn apply_duplicate_fix(
        &self,
        plan: &mut LlmPlan,
        hunk_id: usize,
        chosen_commit_index: usize,
    ) -> Result<(), LlmError> {
        // Remove the hunk from all commits except the chosen one
        for (idx, commit) in plan.commits.iter_mut().enumerate() {
            if idx != chosen_commit_index {
                commit
                    .changes
                    .retain(|change| !matches!(change, ChangeSpec::Hunk { id } if *id == hunk_id));
            }
        }
        eprintln!(
            "  Kept hunk {} only in commit {}",
            hunk_id, chosen_commit_index
        );
        Ok(())
    }

    fn plan_to_commits(
        &self,
        plan: LlmPlan,
        hunks: &[Hunk],
        next_hunk_id: &mut usize,
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        plan.commits
            .into_iter()
            .map(|llm_commit| {
                let changes = llm_commit
                    .changes
                    .into_iter()
                    .map(|spec| -> Result<PlannedChange, ReorganizeError> {
                        match spec {
                            ChangeSpec::Hunk { id } => Ok(PlannedChange::ExistingHunk(HunkId(id))),
                            ChangeSpec::Partial { hunk_id, lines } => {
                                let source =
                                    hunks.iter().find(|h| h.id.0 == hunk_id).ok_or_else(|| {
                                        ReorganizeError::InvalidPlan(format!(
                                            "Hunk {} not found",
                                            hunk_id
                                        ))
                                    })?;
                                let new_hunk = extract_partial_hunk(source, &lines, *next_hunk_id)?;
                                *next_hunk_id += 1;
                                Ok(PlannedChange::NewHunk(new_hunk))
                            }
                            ChangeSpec::Raw { file_path, diff } => {
                                let new_hunk = parse_raw_diff(&file_path, &diff, *next_hunk_id)?;
                                *next_hunk_id += 1;
                                Ok(PlannedChange::NewHunk(new_hunk))
                            }
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(PlannedCommit::new(llm_commit.description, changes))
            })
            .collect()
    }
}

impl Reorganizer for LlmReorganizer {
    fn reorganize(
        &self,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        if hunks.is_empty() {
            return Err(ReorganizeError::NoHunks);
        }
        let plan = self
            .invoke_with_retry(source_commits, hunks)
            .map_err(|e| ReorganizeError::InvalidPlan(e.to_string()))?;
        let mut next_hunk_id = hunks.iter().map(|h| h.id.0).max().unwrap_or(0) + 1;
        self.plan_to_commits(plan, hunks, &mut next_hunk_id)
    }

    fn name(&self) -> &'static str {
        "llm"
    }
}

fn extract_partial_hunk(
    source: &Hunk,
    line_indices: &[usize],
    new_id: usize,
) -> Result<Hunk, ReorganizeError> {
    let mut new_lines = Vec::new();
    let (mut old_count, mut new_count) = (0u32, 0u32);

    for &idx in line_indices {
        if idx == 0 || idx > source.lines.len() {
            return Err(ReorganizeError::InvalidPlan(format!(
                "Invalid line index {} for hunk {}",
                idx, source.id.0
            )));
        }
        let line = &source.lines[idx - 1];
        match line {
            DiffLine::Context(_) => {
                old_count += 1;
                new_count += 1;
            }
            DiffLine::Added(_) => {
                new_count += 1;
            }
            DiffLine::Removed(_) => {
                old_count += 1;
            }
        }
        new_lines.push(line.clone());
    }

    Ok(Hunk {
        id: HunkId(new_id),
        file_path: source.file_path.clone(),
        old_start: source.old_start,
        old_count,
        new_start: source.new_start,
        new_count,
        lines: new_lines,
        likely_source_commits: source.likely_source_commits.clone(),
        old_missing_newline_at_eof: source.old_missing_newline_at_eof,
        new_missing_newline_at_eof: source.new_missing_newline_at_eof,
    })
}

fn parse_raw_diff(file_path: &str, diff: &str, new_id: usize) -> Result<Hunk, ReorganizeError> {
    let (mut old_count, mut new_count) = (0u32, 0u32);
    let lines: Vec<_> = diff
        .lines()
        .filter_map(|line| {
            if let Some(content) = line.strip_prefix('+') {
                new_count += 1;
                Some(DiffLine::Added(content.to_string()))
            } else if let Some(content) = line.strip_prefix('-') {
                old_count += 1;
                Some(DiffLine::Removed(content.to_string()))
            } else if let Some(content) = line.strip_prefix(' ') {
                old_count += 1;
                new_count += 1;
                Some(DiffLine::Context(content.to_string()))
            } else if !line.is_empty() {
                old_count += 1;
                new_count += 1;
                Some(DiffLine::Context(line.to_string()))
            } else {
                None
            }
        })
        .collect();

    if lines.is_empty() {
        return Err(ReorganizeError::InvalidPlan(
            "Raw diff produced no lines".into(),
        ));
    }

    Ok(Hunk {
        id: HunkId(new_id),
        file_path: PathBuf::from(file_path),
        old_start: 1,
        old_count,
        new_start: 1,
        new_count,
        lines,
        likely_source_commits: Vec::new(),
        old_missing_newline_at_eof: false,
        new_missing_newline_at_eof: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_hunk_full;

    #[test]
    fn test_extract_partial_hunk() {
        let source = make_hunk_full(
            0,
            "test.rs",
            vec![
                DiffLine::Context("context".to_string()),
                DiffLine::Added("added".to_string()),
                DiffLine::Removed("removed".to_string()),
            ],
            vec!["abc".to_string()],
        );
        let partial = extract_partial_hunk(&source, &[1, 2], 100).unwrap();

        assert_eq!(partial.id.0, 100);
        assert_eq!(partial.lines.len(), 2);
        assert_eq!(partial.file_path, source.file_path);
    }

    #[test]
    fn test_parse_raw_diff() {
        let diff = "+added line\n-removed line\n context line";
        let hunk = parse_raw_diff("test.rs", diff, 50).unwrap();

        assert_eq!(hunk.id.0, 50);
        assert_eq!(hunk.lines.len(), 3);
        assert_eq!(hunk.new_count, 2); // added + context
        assert_eq!(hunk.old_count, 2); // removed + context
    }
}
