//! Prompt construction for LLM reorganization

use crate::models::{DiffLine, Hunk, SourceCommit};

use super::types::{CommitContext, HunkContext, LlmContext};

/// Build the full context to send to the LLM
pub fn build_context(source_commits: &[SourceCommit], hunks: &[Hunk]) -> LlmContext {
    let commit_contexts: Vec<CommitContext> = source_commits
        .iter()
        .map(|c| CommitContext {
            sha: c.sha.clone(),
            message: c.long_description.clone(),
        })
        .collect();

    let hunk_contexts: Vec<HunkContext> = hunks
        .iter()
        .map(|h| HunkContext {
            id: h.id.0,
            file_path: h.file_path.to_string_lossy().to_string(),
            old_start: h.old_start,
            new_start: h.new_start,
            diff_content: format_diff_lines(&h.lines),
            source_commit_sha: h.likely_source_commits.first().cloned(),
        })
        .collect();

    LlmContext {
        source_commits: commit_contexts,
        hunks: hunk_contexts,
    }
}

/// Format diff lines as a string
fn format_diff_lines(lines: &[DiffLine]) -> String {
    lines
        .iter()
        .map(|line| match line {
            DiffLine::Context(s) => format!(" {}", s),
            DiffLine::Added(s) => format!("+{}", s),
            DiffLine::Removed(s) => format!("-{}", s),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build the prompt to send to the LLM
pub fn build_prompt(context: &LlmContext) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        r#"You are a git commit reorganizer. Your task is to analyze a set of code changes (hunks)
and reorganize them into logical, well-structured commits.

## Input

You will receive:
1. A list of original commits with their messages (for context)
2. A list of hunks (code changes) with their IDs, file paths, and diff content

## Output

You must output a JSON object with the following structure:

```json
{
  "commits": [
    {
      "short_description": "Brief commit message (50 chars or less)",
      "long_description": "Detailed commit message explaining the change",
      "changes": [
        {"type": "hunk", "id": 0},
        {"type": "hunk", "id": 1}
      ]
    }
  ]
}
```

## Change Types

Each change in a commit can be one of:

1. `{"type": "hunk", "id": N}` - Include the entire hunk with ID N
2. `{"type": "partial", "hunk_id": N, "lines": [1, 2, 3]}` - Include only specific lines from hunk N (1-indexed)
3. `{"type": "raw", "file_path": "path/to/file", "diff": "+new line\n-old line"}` - Raw diff content

## Guidelines

1. Group related changes together into logical commits
2. Each commit should represent a single logical change (feature, fix, refactor, etc.)
3. Write clear, descriptive commit messages
4. Prefer smaller, focused commits over large ones
5. Consider file relationships and dependencies when grouping
6. All hunks must be assigned to exactly one commit (no duplicates, no omissions)
7. You may split hunks using "partial" if a hunk contains unrelated changes
8. Preserve the semantic meaning of changes - don't break functionality

## Original Commits (for context)

"#,
    );

    for commit in &context.source_commits {
        prompt.push_str(&format!(
            "### Commit {}\n```\n{}\n```\n\n",
            &commit.sha[..8.min(commit.sha.len())],
            commit.message
        ));
    }

    prompt.push_str("## Hunks to Reorganize\n\n");

    for hunk in &context.hunks {
        prompt.push_str(&format!(
            "### Hunk {} - {}\n",
            hunk.id, hunk.file_path
        ));
        if let Some(ref sha) = hunk.source_commit_sha {
            prompt.push_str(&format!(
                "From commit: {}\n",
                &sha[..8.min(sha.len())]
            ));
        }
        prompt.push_str(&format!(
            "Lines: old @{}, new @{}\n```diff\n{}\n```\n\n",
            hunk.old_start, hunk.new_start, hunk.diff_content
        ));
    }

    prompt.push_str(
        r#"## Your Task

Analyze the hunks above and reorganize them into logical commits. Output ONLY valid JSON matching the schema described above. Do not include any other text or explanation outside the JSON.

```json
"#,
    );

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::HunkId;
    use std::path::PathBuf;

    #[test]
    fn test_build_context() {
        let commits = vec![SourceCommit {
            sha: "abc123".to_string(),
            short_description: "Test commit".to_string(),
            long_description: "Test commit\n\nDetails here".to_string(),
        }];

        let hunks = vec![Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("src/main.rs"),
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 4,
            lines: vec![
                DiffLine::Context("fn main() {".to_string()),
                DiffLine::Added("    println!(\"Hello\");".to_string()),
                DiffLine::Context("}".to_string()),
            ],
            likely_source_commits: vec!["abc123".to_string()],
        }];

        let context = build_context(&commits, &hunks);
        assert_eq!(context.source_commits.len(), 1);
        assert_eq!(context.hunks.len(), 1);
        assert_eq!(context.hunks[0].id, 0);
        assert!(context.hunks[0].diff_content.contains("+    println!"));
    }

    #[test]
    fn test_format_diff_lines() {
        let lines = vec![
            DiffLine::Context("context".to_string()),
            DiffLine::Added("added".to_string()),
            DiffLine::Removed("removed".to_string()),
        ];

        let formatted = format_diff_lines(&lines);
        assert!(formatted.contains(" context"));
        assert!(formatted.contains("+added"));
        assert!(formatted.contains("-removed"));
    }
}
