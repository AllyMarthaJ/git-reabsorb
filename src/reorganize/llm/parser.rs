//! JSON parsing for LLM responses

use std::fs;
use std::path::PathBuf;

use crate::models::{DiffLine, Hunk, HunkId, PlannedChange, PlannedCommit, PlannedCommitId};
use crate::utils::extract_json_str;

use super::types::{ChangeSpec, LlmCommit};
use crate::llm::LlmError;

/// Dump content to a temp file for debugging, returning the path if successful.
fn dump_to_tmp(label: &str, content: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(".git/reabsorb/tmp");
    fs::create_dir_all(&dir).ok()?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let path = dir.join(format!("{}-{}.txt", label, timestamp));
    fs::write(&path, content).ok()?;
    Some(path)
}

/// Wrapper for deserializing the LLM's JSON response
#[derive(serde::Deserialize)]
struct LlmResponse {
    commits: Vec<LlmCommit>,
}

pub fn extract_json(response: &str) -> Result<Vec<LlmCommit>, LlmError> {
    let json_str = match extract_json_str(response) {
        Some(json) => json.trim(),
        None => {
            let detail = match dump_to_tmp("no-json-response", response) {
                Some(path) => format!("Full response dumped to {}", path.display()),
                None => format!(
                    "Response (first 200 chars): {}",
                    response.chars().take(200).collect::<String>()
                ),
            };
            return Err(LlmError::ParseError(format!(
                "No JSON found in response. {}",
                detail
            )));
        }
    };

    let parsed: LlmResponse = serde_json::from_str(json_str).map_err(|e| {
        let detail = match dump_to_tmp("json-parse-error", json_str) {
            Some(path) => format!("Full JSON dumped to {}", path.display()),
            None => format!(
                "JSON (first 200 chars): {}",
                json_str.chars().take(200).collect::<String>()
            ),
        };
        LlmError::ParseError(format!("{}: {}", e, detail))
    })?;
    Ok(parsed.commits)
}

/// Convert LlmCommits to PlannedCommits, processing Partial and Raw specs
pub fn to_planned_commits(
    llm_commits: Vec<LlmCommit>,
    hunks: &[Hunk],
) -> Result<Vec<PlannedCommit>, LlmError> {
    let mut next_hunk_id = hunks.iter().map(|h| h.id.0).max().unwrap_or(0) + 1;

    llm_commits
        .into_iter()
        .enumerate()
        .map(|(commit_idx, llm_commit)| {
            let changes = llm_commit
                .changes
                .into_iter()
                .map(|spec| -> Result<PlannedChange, LlmError> {
                    match spec {
                        ChangeSpec::Hunk { id } => Ok(PlannedChange::ExistingHunk(HunkId(id))),
                        ChangeSpec::Partial { hunk_id, lines } => {
                            let source = hunks
                                .iter()
                                .find(|h| h.id.0 == hunk_id)
                                .ok_or(LlmError::InvalidId(hunk_id))?;
                            let new_hunk = extract_partial_hunk(source, &lines, next_hunk_id)?;
                            next_hunk_id += 1;
                            Ok(PlannedChange::NewHunk(new_hunk))
                        }
                        ChangeSpec::Raw { file_path, diff } => {
                            let new_hunk = parse_raw_diff(&file_path, &diff, next_hunk_id)?;
                            next_hunk_id += 1;
                            Ok(PlannedChange::NewHunk(new_hunk))
                        }
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(PlannedCommit::new(
                PlannedCommitId(commit_idx),
                llm_commit.description,
                changes,
            ))
        })
        .collect()
}

fn extract_partial_hunk(
    source: &Hunk,
    line_indices: &[usize],
    new_id: usize,
) -> Result<Hunk, LlmError> {
    let mut new_lines = Vec::new();
    let (mut old_count, mut new_count) = (0u32, 0u32);

    for &idx in line_indices {
        if idx == 0 || idx > source.lines.len() {
            return Err(LlmError::InvalidIndex {
                item_id: source.id.0,
                index: idx,
            });
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

fn parse_raw_diff(file_path: &str, diff: &str, new_id: usize) -> Result<Hunk, LlmError> {
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
        return Err(LlmError::ValidationError(
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

        let commits = extract_json(response).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].description.short, "Test commit");
    }

    #[test]
    fn test_extract_json_raw() {
        let response = r#"{"commits": [{"short_description": "Test", "long_description": "Test", "changes": []}]}"#;

        let commits = extract_json(response).unwrap();
        assert_eq!(commits.len(), 1);
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
            // Response content is either in a dump file or inline (first 200 chars)
            assert!(
                msg.contains("Running node") || msg.contains("dumped to"),
                "Error should include response content or dump path: {}",
                msg
            );
        }
    }
}
