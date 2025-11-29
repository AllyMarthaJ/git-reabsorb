//! Validator & Repairer - validates commits and repairs issues

use std::collections::HashSet;
use std::sync::Arc;

use crate::models::{CommitDescription, Hunk, HunkId, PlannedChange, PlannedCommit};
use crate::reorganize::llm::LlmClient;

use super::types::{AnalysisResults, ClusterCommit, HierarchicalError};

/// Result of validating a single commit
#[derive(Debug, Clone)]
pub struct CommitValidation {
    pub cluster_id: super::types::ClusterId,
    pub is_valid: bool,
    pub issues: Vec<ValidationIssue>,
}

/// Types of validation issues
#[derive(Debug, Clone)]
pub enum ValidationIssue {
    /// Commit message is empty or too short
    EmptyMessage,
    /// Commit message is too long
    MessageTooLong(usize),
    /// Commit has no changes
    NoChanges,
    /// Commit references invalid hunk
    InvalidHunk(HunkId),
    /// Commit has duplicate hunks
    DuplicateHunk(HunkId),
    /// Commit message doesn't match changes
    MessageMismatch(String),
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMessage => write!(f, "Commit message is empty or too short"),
            Self::MessageTooLong(len) => write!(f, "Commit message too long: {} chars", len),
            Self::NoChanges => write!(f, "Commit has no changes"),
            Self::InvalidHunk(id) => write!(f, "Invalid hunk reference: {}", id.0),
            Self::DuplicateHunk(id) => write!(f, "Duplicate hunk: {}", id.0),
            Self::MessageMismatch(reason) => write!(f, "Message mismatch: {}", reason),
        }
    }
}

/// Validates and repairs commit plans
pub struct Validator {
    client: Option<Arc<dyn LlmClient + Send + Sync>>,
    max_message_length: usize,
    min_message_length: usize,
}

impl Validator {
    pub fn new(client: Option<Arc<dyn LlmClient + Send + Sync>>) -> Self {
        Self {
            client,
            max_message_length: 72,
            min_message_length: 5,
        }
    }

    /// Validate all commits and return validation results
    pub fn validate(&self, commits: &[ClusterCommit], hunks: &[Hunk]) -> Vec<CommitValidation> {
        let valid_hunk_ids: HashSet<HunkId> = hunks.iter().map(|h| h.id).collect();

        commits
            .iter()
            .map(|commit| self.validate_one(commit, &valid_hunk_ids))
            .collect()
    }

    /// Validate a single commit
    fn validate_one(
        &self,
        commit: &ClusterCommit,
        valid_hunk_ids: &HashSet<HunkId>,
    ) -> CommitValidation {
        let mut issues = Vec::new();

        // Check message
        if commit.short_message.trim().len() < self.min_message_length {
            issues.push(ValidationIssue::EmptyMessage);
        }
        if commit.short_message.len() > self.max_message_length {
            issues.push(ValidationIssue::MessageTooLong(commit.short_message.len()));
        }

        // Check changes
        if commit.hunk_ids.is_empty() {
            issues.push(ValidationIssue::NoChanges);
        }

        // Check hunk validity
        let mut seen_hunks = HashSet::new();
        for &hunk_id in &commit.hunk_ids {
            if !valid_hunk_ids.contains(&hunk_id) {
                issues.push(ValidationIssue::InvalidHunk(hunk_id));
            }
            if !seen_hunks.insert(hunk_id) {
                issues.push(ValidationIssue::DuplicateHunk(hunk_id));
            }
        }

        CommitValidation {
            cluster_id: commit.cluster_id,
            is_valid: issues.is_empty(),
            issues,
        }
    }

    /// Validate that all hunks are assigned exactly once
    pub fn validate_complete_assignment(
        &self,
        commits: &[ClusterCommit],
        hunks: &[Hunk],
    ) -> Result<(), HierarchicalError> {
        let all_hunk_ids: HashSet<HunkId> = hunks.iter().map(|h| h.id).collect();
        let mut assigned: HashSet<HunkId> = HashSet::new();

        for commit in commits {
            for &hunk_id in &commit.hunk_ids {
                if !assigned.insert(hunk_id) {
                    return Err(HierarchicalError::ValidationFailed(format!(
                        "Hunk {} assigned to multiple commits",
                        hunk_id.0
                    )));
                }
            }
        }

        let unassigned: Vec<HunkId> = all_hunk_ids.difference(&assigned).copied().collect();
        if !unassigned.is_empty() {
            return Err(HierarchicalError::UnassignedHunks(unassigned));
        }

        Ok(())
    }

    /// Repair invalid commits
    pub fn repair(
        &self,
        commits: Vec<ClusterCommit>,
        validations: &[CommitValidation],
        hunks: &[Hunk],
        analysis: &AnalysisResults,
    ) -> Result<Vec<ClusterCommit>, HierarchicalError> {
        let mut repaired = Vec::new();

        for (commit, validation) in commits.into_iter().zip(validations.iter()) {
            if validation.is_valid {
                repaired.push(commit);
            } else {
                let fixed = self.repair_one(commit, &validation.issues, hunks, analysis)?;
                repaired.push(fixed);
            }
        }

        Ok(repaired)
    }

    /// Repair a single commit
    fn repair_one(
        &self,
        mut commit: ClusterCommit,
        issues: &[ValidationIssue],
        hunks: &[Hunk],
        analysis: &AnalysisResults,
    ) -> Result<ClusterCommit, HierarchicalError> {
        for issue in issues {
            match issue {
                ValidationIssue::EmptyMessage => {
                    // Generate a message from hunk analysis
                    commit.short_message = self.generate_fallback_message(&commit, analysis);
                    if commit.long_message.trim().is_empty() {
                        commit.long_message = commit.short_message.clone();
                    }
                }
                ValidationIssue::MessageTooLong(_) => {
                    // Truncate message
                    commit.short_message = commit
                        .short_message
                        .chars()
                        .take(self.max_message_length - 3)
                        .collect::<String>()
                        + "...";
                }
                ValidationIssue::NoChanges => {
                    // Can't repair - this commit should be removed
                    return Err(HierarchicalError::ValidationFailed(
                        "Commit has no changes and cannot be repaired".to_string(),
                    ));
                }
                ValidationIssue::InvalidHunk(hunk_id) => {
                    // Remove invalid hunk reference
                    commit.hunk_ids.retain(|&id| id != *hunk_id);
                }
                ValidationIssue::DuplicateHunk(hunk_id) => {
                    // Remove duplicates, keep first occurrence
                    let mut seen = HashSet::new();
                    commit.hunk_ids.retain(|&id| {
                        if id == *hunk_id {
                            seen.insert(id)
                        } else {
                            true
                        }
                    });
                }
                ValidationIssue::MessageMismatch(_) => {
                    // Try to regenerate message with LLM if available
                    if let Some(client) = &self.client {
                        if let Ok(new_message) =
                            self.regenerate_message(client, &commit, hunks, analysis)
                        {
                            commit.short_message = new_message.0;
                            commit.long_message = new_message.1;
                        }
                    }
                }
            }
        }

        Ok(commit)
    }

    /// Generate a fallback message from hunk analysis
    fn generate_fallback_message(
        &self,
        commit: &ClusterCommit,
        analysis: &AnalysisResults,
    ) -> String {
        let semantic_units: Vec<&str> = commit
            .hunk_ids
            .iter()
            .filter_map(|id| analysis.get(*id))
            .flat_map(|a| a.semantic_units.iter().map(|s| s.as_str()))
            .take(3)
            .collect();

        if semantic_units.is_empty() {
            "Update code".to_string()
        } else if semantic_units.len() == 1 {
            capitalize_first(semantic_units[0])
        } else {
            format!(
                "{} and {}",
                capitalize_first(semantic_units[0]),
                semantic_units[1..].join(", ")
            )
        }
    }

    /// Regenerate message using LLM
    fn regenerate_message(
        &self,
        client: &Arc<dyn LlmClient + Send + Sync>,
        commit: &ClusterCommit,
        hunks: &[Hunk],
        analysis: &AnalysisResults,
    ) -> Result<(String, String), String> {
        let prompt = build_repair_prompt(commit, hunks, analysis);

        let response = client
            .complete(&prompt)
            .map_err(|e| format!("LLM error: {}", e))?;

        parse_repair_response(&response)
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn build_repair_prompt(
    commit: &ClusterCommit,
    _hunks: &[Hunk],
    analysis: &AnalysisResults,
) -> String {
    let mut prompt = String::from("Write a better commit message for these changes:\n\n");

    prompt.push_str(&format!("Current message: {}\n\n", commit.short_message));

    prompt.push_str("Changes:\n");
    for &hunk_id in &commit.hunk_ids {
        if let Some(a) = analysis.get(hunk_id) {
            prompt.push_str(&format!(
                "- {} ({}): {}\n",
                a.file_path,
                a.category,
                a.semantic_units.join(", ")
            ));
        }
    }

    prompt.push_str(
        r#"
Respond with ONLY JSON:
{
  "short_message": "Concise commit message (max 72 chars)",
  "long_message": "Detailed explanation"
}"#,
    );

    prompt
}

fn parse_repair_response(response: &str) -> Result<(String, String), String> {
    let json_str = if let Some(start) = response.find('{') {
        let end = response.rfind('}').unwrap_or(response.len());
        &response[start..=end]
    } else {
        return Err("No JSON found in repair response".to_string());
    };

    #[derive(serde::Deserialize)]
    struct RepairResponse {
        short_message: String,
        long_message: String,
    }

    let parsed: RepairResponse =
        serde_json::from_str(json_str).map_err(|e| format!("Failed to parse: {}", e))?;

    Ok((parsed.short_message, parsed.long_message))
}

/// Convert ClusterCommits to PlannedCommits
pub fn to_planned_commits(commits: Vec<ClusterCommit>) -> Vec<PlannedCommit> {
    commits
        .into_iter()
        .map(|c| {
            let changes = c
                .hunk_ids
                .into_iter()
                .map(PlannedChange::ExistingHunk)
                .collect();

            PlannedCommit::new(
                CommitDescription::new(c.short_message, c.long_message),
                changes,
            )
        })
        .collect()
}

/// Assign orphaned hunks to existing commits or create new ones
pub fn assign_orphans(
    mut commits: Vec<ClusterCommit>,
    hunks: &[Hunk],
    analysis: &AnalysisResults,
) -> Vec<ClusterCommit> {
    let assigned: HashSet<HunkId> = commits
        .iter()
        .flat_map(|c| c.hunk_ids.iter().copied())
        .collect();

    let orphans: Vec<HunkId> = hunks
        .iter()
        .map(|h| h.id)
        .filter(|id| !assigned.contains(id))
        .collect();

    if orphans.is_empty() {
        return commits;
    }

    // Try to assign orphans to existing commits with matching topics
    let mut remaining_orphans = Vec::new();

    for orphan_id in orphans {
        let orphan_topic = analysis
            .get(orphan_id)
            .map(|a| a.topic.as_str())
            .unwrap_or("");

        // Find a commit with matching topic
        let matching_commit = commits.iter_mut().find(|c| {
            c.hunk_ids
                .iter()
                .any(|id| analysis.get(*id).map(|a| a.topic.as_str()) == Some(orphan_topic))
        });

        if let Some(commit) = matching_commit {
            commit.hunk_ids.push(orphan_id);
        } else {
            remaining_orphans.push(orphan_id);
        }
    }

    // Create a catch-all commit for remaining orphans
    if !remaining_orphans.is_empty() {
        let next_id = commits.iter().map(|c| c.cluster_id.0).max().unwrap_or(0) + 1;

        commits.push(ClusterCommit {
            cluster_id: super::types::ClusterId(next_id),
            short_message: "Additional changes".to_string(),
            long_message: "Miscellaneous changes that didn't fit elsewhere".to_string(),
            hunk_ids: remaining_orphans,
            depends_on: Vec::new(),
        });
    }

    commits
}

#[cfg(test)]
mod tests {
    use super::super::types::{ChangeCategory, ClusterId, HunkAnalysis};
    use super::*;
    use crate::test_utils::make_hunk;

    fn make_commit(id: usize, message: &str, hunk_ids: Vec<usize>) -> ClusterCommit {
        ClusterCommit {
            cluster_id: ClusterId(id),
            short_message: message.to_string(),
            long_message: message.to_string(),
            hunk_ids: hunk_ids.into_iter().map(HunkId).collect(),
            depends_on: Vec::new(),
        }
    }

    #[test]
    fn test_validate_empty_message() {
        let validator = Validator::new(None);
        let hunks = vec![make_hunk(0)];
        let commit = make_commit(0, "", vec![0]);

        let validations = validator.validate(&[commit], &hunks);

        assert!(!validations[0].is_valid);
        assert!(validations[0]
            .issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::EmptyMessage)));
    }

    #[test]
    fn test_validate_long_message() {
        let validator = Validator::new(None);
        let hunks = vec![make_hunk(0)];
        let long_message = "x".repeat(100);
        let commit = make_commit(0, &long_message, vec![0]);

        let validations = validator.validate(&[commit], &hunks);

        assert!(!validations[0].is_valid);
        assert!(validations[0]
            .issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::MessageTooLong(_))));
    }

    #[test]
    fn test_validate_invalid_hunk() {
        let validator = Validator::new(None);
        let hunks = vec![make_hunk(0)];
        let commit = make_commit(0, "Valid message", vec![0, 999]); // 999 doesn't exist

        let validations = validator.validate(&[commit], &hunks);

        assert!(!validations[0].is_valid);
        assert!(validations[0]
            .issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::InvalidHunk(HunkId(999)))));
    }

    #[test]
    fn test_validate_duplicate_hunk() {
        let validator = Validator::new(None);
        let hunks = vec![make_hunk(0)];
        let commit = make_commit(0, "Valid message", vec![0, 0]); // Duplicate

        let validations = validator.validate(&[commit], &hunks);

        assert!(!validations[0].is_valid);
        assert!(validations[0]
            .issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::DuplicateHunk(_))));
    }

    #[test]
    fn test_validate_complete_assignment() {
        let validator = Validator::new(None);
        let hunks = vec![make_hunk(0), make_hunk(1)];
        let commits = vec![make_commit(0, "Valid", vec![0])]; // Missing hunk 1

        let result = validator.validate_complete_assignment(&commits, &hunks);

        assert!(matches!(result, Err(HierarchicalError::UnassignedHunks(_))));
    }

    #[test]
    fn test_assign_orphans() {
        let hunks = vec![make_hunk(0), make_hunk(1), make_hunk(2)];
        let commits = vec![make_commit(0, "Valid", vec![0])]; // Missing 1 and 2

        let mut analysis = AnalysisResults::new();
        analysis.add(HunkAnalysis {
            hunk_id: 0,
            category: ChangeCategory::Feature,
            semantic_units: vec![],
            topic: "auth".to_string(),
            depends_on_context: None,
            file_path: "test.rs".to_string(),
        });
        analysis.add(HunkAnalysis {
            hunk_id: 1,
            category: ChangeCategory::Feature,
            semantic_units: vec![],
            topic: "auth".to_string(), // Same topic as 0
            depends_on_context: None,
            file_path: "test.rs".to_string(),
        });
        analysis.add(HunkAnalysis {
            hunk_id: 2,
            category: ChangeCategory::Feature,
            semantic_units: vec![],
            topic: "other".to_string(), // Different topic
            depends_on_context: None,
            file_path: "test.rs".to_string(),
        });

        let result = assign_orphans(commits, &hunks, &analysis);

        // Hunk 1 should be added to existing commit, hunk 2 to new commit
        assert_eq!(result.len(), 2);
        assert!(result[0].hunk_ids.contains(&HunkId(0)));
        assert!(result[0].hunk_ids.contains(&HunkId(1)));
        assert!(result[1].hunk_ids.contains(&HunkId(2)));
    }

    #[test]
    fn test_to_planned_commits() {
        let commits = vec![
            make_commit(0, "First commit", vec![0, 1]),
            make_commit(1, "Second commit", vec![2]),
        ];

        let planned = to_planned_commits(commits);

        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].description.short, "First commit");
        assert_eq!(planned[0].changes.len(), 2);
        assert_eq!(planned[1].changes.len(), 1);
    }
}
