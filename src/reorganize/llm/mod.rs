//! LLM-based reorganization strategy

mod client;
mod parser;
mod prompt;
mod types;

pub use client::{ClaudeCliClient, LlmClient};
pub use types::{ChangeSpec, LlmError, LlmPlan};

use std::path::PathBuf;

use crate::models::{DiffLine, Hunk, HunkId, PlannedChange, PlannedCommit, SourceCommit};
use crate::reorganize::{ReorganizeError, Reorganizer};

/// LLM-based reorganizer that uses AI to intelligently group changes
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

    /// Invoke the LLM with retry logic
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
                Ok(response) => {
                    match parser::extract_json(&response) {
                        Ok(plan) => {
                            match parser::validate_plan(&plan, hunks) {
                                Ok(()) => return Ok(plan),
                                Err(e) => {
                                    eprintln!("Validation error: {}", e);
                                    last_error = Some(e);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Parse error: {}", e);
                            last_error = Some(e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Client error: {}", e);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(LlmError::MaxRetriesExceeded(self.max_retries)))
    }

    /// Convert LLM plan to PlannedCommits
    fn plan_to_commits(
        &self,
        plan: LlmPlan,
        hunks: &[Hunk],
        next_hunk_id: &mut usize,
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        let mut planned_commits = Vec::new();

        for llm_commit in plan.commits {
            let mut changes = Vec::new();

            for change_spec in llm_commit.changes {
                match change_spec {
                    ChangeSpec::Hunk { id } => {
                        changes.push(PlannedChange::ExistingHunk(HunkId(id)));
                    }
                    ChangeSpec::Partial { hunk_id, lines } => {
                        // Create a new hunk with only the specified lines
                        let source_hunk = hunks
                            .iter()
                            .find(|h| h.id.0 == hunk_id)
                            .ok_or_else(|| {
                                ReorganizeError::InvalidPlan(format!(
                                    "Hunk {} not found",
                                    hunk_id
                                ))
                            })?;

                        let new_hunk = extract_partial_hunk(source_hunk, &lines, *next_hunk_id)?;
                        *next_hunk_id += 1;
                        changes.push(PlannedChange::NewHunk(new_hunk));
                    }
                    ChangeSpec::Raw { file_path, diff } => {
                        let new_hunk = parse_raw_diff(&file_path, &diff, *next_hunk_id)?;
                        *next_hunk_id += 1;
                        changes.push(PlannedChange::NewHunk(new_hunk));
                    }
                }
            }

            planned_commits.push(PlannedCommit::new(
                llm_commit.description,
                changes,
            ));
        }

        Ok(planned_commits)
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

        // Start new hunk IDs after the existing ones
        let mut next_hunk_id = hunks.iter().map(|h| h.id.0).max().unwrap_or(0) + 1;

        self.plan_to_commits(plan, hunks, &mut next_hunk_id)
    }

    fn name(&self) -> &'static str {
        "llm"
    }
}

/// Extract specific lines from a hunk to create a new partial hunk
fn extract_partial_hunk(
    source: &Hunk,
    line_indices: &[usize],
    new_id: usize,
) -> Result<Hunk, ReorganizeError> {
    // Line indices are 1-indexed
    let mut new_lines = Vec::new();
    let mut old_count = 0u32;
    let mut new_count = 0u32;

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
    })
}

/// Parse raw diff content into a hunk
fn parse_raw_diff(
    file_path: &str,
    diff: &str,
    new_id: usize,
) -> Result<Hunk, ReorganizeError> {
    let mut lines = Vec::new();
    let mut old_count = 0u32;
    let mut new_count = 0u32;

    for line in diff.lines() {
        if let Some(content) = line.strip_prefix('+') {
            lines.push(DiffLine::Added(content.to_string()));
            new_count += 1;
        } else if let Some(content) = line.strip_prefix('-') {
            lines.push(DiffLine::Removed(content.to_string()));
            old_count += 1;
        } else if let Some(content) = line.strip_prefix(' ') {
            lines.push(DiffLine::Context(content.to_string()));
            old_count += 1;
            new_count += 1;
        } else if !line.is_empty() {
            // Treat as context line without prefix
            lines.push(DiffLine::Context(line.to_string()));
            old_count += 1;
            new_count += 1;
        }
    }

    if lines.is_empty() {
        return Err(ReorganizeError::InvalidPlan(
            "Raw diff produced no lines".to_string(),
        ));
    }

    Ok(Hunk {
        id: HunkId(new_id),
        file_path: PathBuf::from(file_path),
        old_start: 1, // We don't know the exact position for raw diffs
        old_count,
        new_start: 1,
        new_count,
        lines,
        likely_source_commits: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_hunk(id: usize) -> Hunk {
        Hunk {
            id: HunkId(id),
            file_path: PathBuf::from("test.rs"),
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 3,
            lines: vec![
                DiffLine::Context("context".to_string()),
                DiffLine::Added("added".to_string()),
                DiffLine::Removed("removed".to_string()),
            ],
            likely_source_commits: vec!["abc".to_string()],
        }
    }

    #[test]
    fn test_extract_partial_hunk() {
        let source = make_test_hunk(0);
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
