//! JSON parsing and validation for LLM responses

use std::collections::HashSet;

use crate::models::Hunk;

use super::types::{ChangeSpec, LlmError, LlmPlan};

pub fn extract_json(response: &str) -> Result<LlmPlan, LlmError> {
    let json_str = if let Some(start) = response.find("```json") {
        let content_start = start + 7;
        let end = response[content_start..]
            .find("```")
            .map(|e| content_start + e)
            .unwrap_or(response.len());
        &response[content_start..end]
    } else if let Some(start) = response.find("```") {
        let content_start = start + 3;
        // Skip any language identifier on the same line
        let content_start = response[content_start..]
            .find('\n')
            .map(|n| content_start + n + 1)
            .unwrap_or(content_start);
        let end = response[content_start..]
            .find("```")
            .map(|e| content_start + e)
            .unwrap_or(response.len());
        &response[content_start..end]
    } else if let Some(start) = response.find('{') {
        // Try to find raw JSON starting with {
        let end = response.rfind('}').map(|e| e + 1).unwrap_or(response.len());
        &response[start..end]
    } else {
        response
    };

    let json_str = json_str.trim();

    serde_json::from_str(json_str)
        .map_err(|e| LlmError::ParseError(format!("{}: {}", e, truncate(json_str, 200))))
}

fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() > max_len {
        &s[..max_len]
    } else {
        s
    }
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
                        return Err(LlmError::InvalidHunkId(*id));
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
                        return Err(LlmError::InvalidHunkId(*hunk_id));
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
                            return Err(LlmError::InvalidLineIndex {
                                hunk_id: *hunk_id,
                                line,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DiffLine, HunkId};
    use std::path::PathBuf;

    fn make_test_hunk(id: usize) -> Hunk {
        Hunk {
            id: HunkId(id),
            file_path: PathBuf::from("test.rs"),
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 3,
            lines: vec![
                DiffLine::Context("line1".to_string()),
                DiffLine::Added("line2".to_string()),
                DiffLine::Removed("line3".to_string()),
            ],
            likely_source_commits: vec!["abc123".to_string()],
        }
    }

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
        let hunks = vec![make_test_hunk(0), make_test_hunk(1)];
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
        let hunks = vec![make_test_hunk(0)];
        let plan = LlmPlan {
            commits: vec![make_llm_commit(
                "Test",
                "Test",
                vec![ChangeSpec::Hunk { id: 999 }],
            )],
        };

        let result = validate_plan(&plan, &hunks);
        assert!(matches!(result, Err(LlmError::InvalidHunkId(999))));
    }

    #[test]
    fn test_validate_plan_duplicate_hunk() {
        let hunks = vec![make_test_hunk(0)];
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
        let hunks = vec![make_test_hunk(0), make_test_hunk(1)];
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
