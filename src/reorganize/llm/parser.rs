//! JSON parsing and validation for LLM responses

use std::collections::HashSet;

use crate::models::Hunk;
use crate::utils::extract_json_str;

use super::types::{ChangeSpec, LlmPlan};
use crate::llm::LlmError;

/// Specific validation issues that can be fixed with targeted prompts
#[derive(Debug, Clone)]
pub enum ValidationIssue {
    /// Hunks that weren't assigned to any commit
    UnassignedHunks(Vec<usize>),
    /// A hunk assigned to multiple commits (hunk_id, commit indices)
    DuplicateHunk {
        hunk_id: usize,
        commit_indices: Vec<usize>,
    },
}

pub fn extract_json(response: &str) -> Result<LlmPlan, LlmError> {
    let json_str = match extract_json_str(response) {
        Some(json) => json.trim(),
        None => {
            return Err(LlmError::ParseError(format!(
                "No JSON found in response. Response content: {}",
                response
            )));
        }
    };

    serde_json::from_str(json_str).map_err(|e| LlmError::ParseError(format!("{}: {}", e, json_str)))
}

pub fn validate_plan(plan: &LlmPlan, hunks: &[Hunk]) -> Result<(), LlmError> {
    let valid_hunk_ids: HashSet<usize> = hunks.iter().map(|h| h.id.0).collect();
    let mut assigned_hunks: HashSet<usize> = HashSet::new();

    for commit in &plan.commits {
        if commit.description.short.is_empty() {
            return Err(LlmError::ValidationError(
                "Commit has empty short description".to_string(),
            ));
        }

        for change in &commit.changes {
            match change {
                ChangeSpec::Hunk { id } => {
                    if !valid_hunk_ids.contains(id) {
                        return Err(LlmError::InvalidId(*id));
                    }
                    if !assigned_hunks.insert(*id) {
                        return Err(LlmError::ValidationError(format!(
                            "Hunk {} assigned to multiple commits",
                            id
                        )));
                    }
                }
                ChangeSpec::Partial { hunk_id, lines } => {
                    if !valid_hunk_ids.contains(hunk_id) {
                        return Err(LlmError::InvalidId(*hunk_id));
                    }
                    if lines.is_empty() {
                        return Err(LlmError::ValidationError(format!(
                            "Partial hunk {} has no lines",
                            hunk_id
                        )));
                    }
                    let hunk = hunks.iter().find(|h| h.id.0 == *hunk_id).unwrap();
                    let max_line = hunk.lines.len();
                    for &line in lines {
                        if line == 0 || line > max_line {
                            return Err(LlmError::InvalidIndex {
                                item_id: *hunk_id,
                                index: line,
                            });
                        }
                    }
                }
                ChangeSpec::Raw { file_path, diff } => {
                    if file_path.is_empty() {
                        return Err(LlmError::ValidationError(
                            "Raw change has empty file_path".to_string(),
                        ));
                    }
                    if diff.is_empty() {
                        return Err(LlmError::ValidationError(
                            "Raw change has empty diff".to_string(),
                        ));
                    }
                }
            }
        }
    }

    let unassigned: Vec<_> = valid_hunk_ids
        .difference(&assigned_hunks)
        .copied()
        .collect();
    if !unassigned.is_empty() {
        return Err(LlmError::ValidationError(format!(
            "{} hunks were not assigned to any commit: {:?}",
            unassigned.len(),
            unassigned
        )));
    }

    Ok(())
}

/// Validate and return specific fixable issues if any
pub fn validate_plan_with_issues(
    plan: &LlmPlan,
    hunks: &[Hunk],
) -> Result<(), (LlmError, Option<ValidationIssue>)> {
    let valid_hunk_ids: HashSet<usize> = hunks.iter().map(|h| h.id.0).collect();
    let mut hunk_to_commits: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();

    for (commit_idx, commit) in plan.commits.iter().enumerate() {
        if commit.description.short.is_empty() {
            return Err((
                LlmError::ValidationError("Commit has empty short description".to_string()),
                None,
            ));
        }

        for change in &commit.changes {
            match change {
                ChangeSpec::Hunk { id } => {
                    if !valid_hunk_ids.contains(id) {
                        return Err((LlmError::InvalidId(*id), None));
                    }
                    hunk_to_commits.entry(*id).or_default().push(commit_idx);
                }
                ChangeSpec::Partial { hunk_id, lines } => {
                    if !valid_hunk_ids.contains(hunk_id) {
                        return Err((LlmError::InvalidId(*hunk_id), None));
                    }
                    if lines.is_empty() {
                        return Err((
                            LlmError::ValidationError(format!(
                                "Partial hunk {} has no lines",
                                hunk_id
                            )),
                            None,
                        ));
                    }
                    let hunk = hunks.iter().find(|h| h.id.0 == *hunk_id).unwrap();
                    let max_line = hunk.lines.len();
                    for &line in lines {
                        if line == 0 || line > max_line {
                            return Err((
                                LlmError::InvalidIndex {
                                    item_id: *hunk_id,
                                    index: line,
                                },
                                None,
                            ));
                        }
                    }
                    // Partial hunks don't count as "assigned" for duplicate detection
                }
                ChangeSpec::Raw { file_path, diff } => {
                    if file_path.is_empty() {
                        return Err((
                            LlmError::ValidationError("Raw change has empty file_path".to_string()),
                            None,
                        ));
                    }
                    if diff.is_empty() {
                        return Err((
                            LlmError::ValidationError("Raw change has empty diff".to_string()),
                            None,
                        ));
                    }
                }
            }
        }
    }

    // Check for duplicates
    for (hunk_id, commits) in &hunk_to_commits {
        if commits.len() > 1 {
            return Err((
                LlmError::ValidationError(format!("Hunk {} assigned to multiple commits", hunk_id)),
                Some(ValidationIssue::DuplicateHunk {
                    hunk_id: *hunk_id,
                    commit_indices: commits.clone(),
                }),
            ));
        }
    }

    // Check for unassigned
    let assigned_hunks: HashSet<usize> = hunk_to_commits.keys().copied().collect();
    let unassigned: Vec<_> = valid_hunk_ids
        .difference(&assigned_hunks)
        .copied()
        .collect();
    if !unassigned.is_empty() {
        return Err((
            LlmError::ValidationError(format!(
                "{} hunks were not assigned to any commit: {:?}",
                unassigned.len(),
                unassigned
            )),
            Some(ValidationIssue::UnassignedHunks(unassigned)),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::make_hunk;

    #[test]
    fn test_extract_json_with_code_fence() {
        let response = r#"Here's the plan:

```json
{
  "commits": [
    {
      "short_description": "Test commit",
      "long_description": "Test",
      "changes": [{"type": "hunk", "id": 0}]
    }
  ]
}
```

That's it!"#;

        let plan = extract_json(response).unwrap();
        assert_eq!(plan.commits.len(), 1);
        assert_eq!(plan.commits[0].description.short, "Test commit");
    }

    #[test]
    fn test_extract_json_raw() {
        let response = r#"{"commits": [{"short_description": "Test", "long_description": "Test", "changes": []}]}"#;

        let plan = extract_json(response).unwrap();
        assert_eq!(plan.commits.len(), 1);
    }

    fn make_llm_commit(
        short: &str,
        long: &str,
        changes: Vec<ChangeSpec>,
    ) -> super::super::types::LlmCommit {
        super::super::types::LlmCommit {
            description: crate::models::CommitDescription::new(short, long),
            changes,
        }
    }

    #[test]
    fn test_validate_plan_valid() {
        let hunks = vec![make_hunk(0), make_hunk(1)];
        let plan = LlmPlan {
            commits: vec![make_llm_commit(
                "Test",
                "Test commit",
                vec![ChangeSpec::Hunk { id: 0 }, ChangeSpec::Hunk { id: 1 }],
            )],
        };

        assert!(validate_plan(&plan, &hunks).is_ok());
    }

    #[test]
    fn test_validate_plan_invalid_hunk_id() {
        let hunks = vec![make_hunk(0)];
        let plan = LlmPlan {
            commits: vec![make_llm_commit(
                "Test",
                "Test",
                vec![ChangeSpec::Hunk { id: 999 }],
            )],
        };

        let result = validate_plan(&plan, &hunks);
        assert!(matches!(result, Err(LlmError::InvalidId(999))));
    }

    #[test]
    fn test_validate_plan_duplicate_hunk() {
        let hunks = vec![make_hunk(0)];
        let plan = LlmPlan {
            commits: vec![make_llm_commit(
                "Test",
                "Test",
                vec![ChangeSpec::Hunk { id: 0 }, ChangeSpec::Hunk { id: 0 }],
            )],
        };

        let result = validate_plan(&plan, &hunks);
        assert!(matches!(result, Err(LlmError::ValidationError(_))));
    }

    #[test]
    fn test_validate_plan_unassigned_hunks() {
        // Two hunks exist but only one is assigned - should error so we can retry
        let hunks = vec![make_hunk(0), make_hunk(1)];
        let plan = LlmPlan {
            commits: vec![make_llm_commit(
                "Test",
                "Test",
                vec![ChangeSpec::Hunk { id: 0 }], // Missing hunk 1
            )],
        };

        let result = validate_plan(&plan, &hunks);
        assert!(matches!(result, Err(LlmError::ValidationError(_))));
        if let Err(LlmError::ValidationError(msg)) = result {
            assert!(
                msg.contains("not assigned"),
                "Error should mention unassigned hunks: {}",
                msg
            );
        }
    }

    #[test]
    fn test_extract_json_no_json_found() {
        let response = "Running node v24.8.0 (npm v11.6.0)";
        let result = extract_json(response);
        assert!(matches!(result, Err(LlmError::ParseError(_))));
        if let Err(LlmError::ParseError(msg)) = result {
            assert!(
                msg.contains("No JSON found"),
                "Error should mention no JSON found: {}",
                msg
            );
            assert!(
                msg.contains("Running node"),
                "Error should include the response content: {}",
                msg
            );
        }
    }
}
