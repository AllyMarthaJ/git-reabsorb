//! Prompt construction for LLM reorganization

use std::path::Path;

use crate::models::{Hunk, SourceCommit};
use crate::utils::format_diff_lines;

use super::types::{CommitContext, HunkContext, LlmContext};

pub fn build_context(source_commits: &[SourceCommit], hunks: &[Hunk]) -> LlmContext {
    let commit_contexts: Vec<CommitContext> = source_commits
        .iter()
        .map(|c| CommitContext {
            source_commit: c.clone(),
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
            source_commit_shas: h.likely_source_commits.clone(),
        })
        .collect();

    LlmContext {
        source_commits: commit_contexts,
        hunks: hunk_contexts,
    }
}

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

0. You should ALWAYS emphasise WHY a change was made, if that information was available.
   The short description should contain a concise summary, and the long description
   should elaborate on the reasoning. Avoid generic messages and short descriptions beginning
   with "Add", "Fix", "Update", etc. without context.
1. Group related changes together into logical commits
2. Each commit should represent a single logical change (feature, fix, refactor, etc.)
3. Write clear, descriptive commit messages
4. Consider file relationships and dependencies when grouping
5. All hunks must be assigned to exactly one commit (no duplicates, no omissions)
6. You may split hunks using "partial" if a hunk contains unrelated changes
7. Preserve the semantic meaning of changes - don't break functionality

## Original Commits (for context)

"#,
    );

    for commit in &context.source_commits {
        prompt.push_str(&format!(
            "### Commit {}\n```\n{}\n```\n\n",
            &commit.source_commit.sha[..8.min(commit.source_commit.sha.len())],
            commit.source_commit.message.long
        ));
    }

    prompt.push_str("## Hunks to Reorganize\n\n");

    for hunk in &context.hunks {
        prompt.push_str(&format!("### Hunk {} - {}\n", hunk.id, hunk.file_path));
        if !hunk.source_commit_shas.is_empty() {
            prompt.push_str("Source commits:\n");
            for sha in &hunk.source_commit_shas {
                let commit_msg = context
                    .source_commits
                    .iter()
                    .find(|c| {
                        c.source_commit.sha.starts_with(sha)
                            || sha.starts_with(&c.source_commit.sha)
                    })
                    .map(|c| c.source_commit.message.long.as_str())
                    .unwrap_or("(unknown)");
                prompt.push_str(&format!(
                    "  - {} - {}\n",
                    &sha[..8.min(sha.len())],
                    commit_msg.lines().next().unwrap_or("(no message)")
                ));
            }
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

/// Build the content for the hunks input file (file-based I/O mode).
///
/// This extracts the hunks section that would normally be embedded in the prompt.
pub fn build_hunks_file_content(context: &LlmContext) -> String {
    let mut content = String::new();

    content.push_str("# Hunks to Reorganize\n\n");

    for hunk in &context.hunks {
        content.push_str(&format!("## Hunk {} - {}\n", hunk.id, hunk.file_path));
        if !hunk.source_commit_shas.is_empty() {
            content.push_str("Source commits:\n");
            for sha in &hunk.source_commit_shas {
                let commit_msg = context
                    .source_commits
                    .iter()
                    .find(|c| {
                        c.source_commit.sha.starts_with(sha)
                            || sha.starts_with(&c.source_commit.sha)
                    })
                    .map(|c| c.source_commit.message.long.as_str())
                    .unwrap_or("(unknown)");
                content.push_str(&format!(
                    "  - {} - {}\n",
                    &sha[..8.min(sha.len())],
                    commit_msg.lines().next().unwrap_or("(no message)")
                ));
            }
        }
        content.push_str(&format!(
            "Lines: old @{}, new @{}\n```diff\n{}\n```\n\n",
            hunk.old_start, hunk.new_start, hunk.diff_content
        ));
    }

    content
}

/// Build a prompt that references an external file for input and asks Claude to write output to a file.
///
/// This reduces token usage by storing hunks in a file rather than embedding them in the prompt.
/// Claude writes its response to a file of its choosing and outputs the path to stdout.
pub fn build_file_based_prompt(context: &LlmContext, input_file_path: &Path) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        r#"You are a git commit reorganizer. Your task is to analyze a set of code changes (hunks)
and reorganize them into logical, well-structured commits.

## Input

You will receive:
1. A list of original commits with their messages (for context, below)
2. A list of hunks (code changes) in an EXTERNAL FILE (path provided below)

## Output

You must:
1. Write a JSON file containing your reorganization plan
2. Output ONLY a single line to stdout with the absolute path to that file

The JSON file should contain:

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

0. You should ALWAYS emphasise WHY a change was made, if that information was available.
   The short description should contain a concise summary, and the long description
   should elaborate on the reasoning. Avoid generic messages and short descriptions beginning
   with "Add", "Fix", "Update", etc. without context.
1. Group related changes together into logical commits
2. Each commit should represent a single logical change (feature, fix, refactor, etc.)
3. Write clear, descriptive commit messages
4. Consider file relationships and dependencies when grouping
5. All hunks must be assigned to exactly one commit (no duplicates, no omissions)
6. You may split hunks using "partial" if a hunk contains unrelated changes
7. Preserve the semantic meaning of changes - don't break functionality

## Original Commits (for context)

"#,
    );

    for commit in &context.source_commits {
        prompt.push_str(&format!(
            "### Commit {}\n```\n{}\n```\n\n",
            &commit.source_commit.sha[..8.min(commit.source_commit.sha.len())],
            commit.source_commit.message.long
        ));
    }

    prompt.push_str(&format!(
        r#"## Hunks File

Read the hunks from this file: {}

The file contains all hunk details including IDs, file paths, source commits, and diff content.

## Your Task

1. Read the hunks from the input file
2. Analyze and reorganize them into logical commits
3. Write your JSON response to a file
4. Output ONLY the absolute path to that file (nothing else)
"#,
        input_file_path.display()
    ));

    prompt
}

/// Build a prompt to fix unassigned hunks by adding them to existing or new commits
pub fn build_fix_unassigned_prompt(
    context: &LlmContext,
    commits: &[crate::models::PlannedCommit],
    unassigned_hunk_ids: &[usize],
) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        r#"You previously created a commit plan, but some hunks were not assigned to any commit.
Please assign the missing hunks to existing commits or create new commits for them.

## Current Commits

"#,
    );

    // Show current commits
    for (idx, commit) in commits.iter().enumerate() {
        prompt.push_str(&format!(
            "### Commit {} - \"{}\"\n{}\n\n",
            idx, commit.description.short, commit.description.long
        ));
    }

    prompt.push_str("## Unassigned Hunks\n\n");

    // Show details of unassigned hunks
    for hunk_id in unassigned_hunk_ids {
        if let Some(hunk) = context.hunks.iter().find(|h| h.id == *hunk_id) {
            prompt.push_str(&format!("### Hunk {} - {}\n", hunk.id, hunk.file_path));
            if !hunk.source_commit_shas.is_empty() {
                prompt.push_str("Source commits:\n");
                for sha in &hunk.source_commit_shas {
                    let commit_msg = context
                        .source_commits
                        .iter()
                        .find(|c| {
                            c.source_commit.sha.starts_with(sha)
                                || sha.starts_with(&c.source_commit.sha)
                        })
                        .map(|c| c.source_commit.message.long.as_str())
                        .unwrap_or("(unknown)");
                    prompt.push_str(&format!(
                        "  - {} - {}\n",
                        &sha[..8.min(sha.len())],
                        commit_msg.lines().next().unwrap_or("(no message)")
                    ));
                }
            }
            prompt.push_str(&format!(
                "Lines: old @{}, new @{}\n```diff\n{}\n```\n\n",
                hunk.old_start, hunk.new_start, hunk.diff_content
            ));
        }
    }

    prompt.push_str(
        r#"## Your Task

For each unassigned hunk, decide:
1. Add it to an existing commit (specify the commit's short_description)
2. Create a new commit for it

Output a JSON object with the assignments:

```json
{
  "assignments": [
    {"hunk_id": N, "action": "add_to_existing", "commit_description": "existing commit short description"},
    {"hunk_id": M, "action": "new_commit", "short_description": "New commit message", "long_description": "Details"}
  ]
}
```

Output ONLY the JSON.

```json
"#,
    );

    prompt
}

/// Build a prompt to resolve a duplicate hunk assignment
pub fn build_fix_duplicate_prompt(
    context: &LlmContext,
    commits: &[crate::models::PlannedCommit],
    hunk_id: usize,
    commit_indices: &[usize],
) -> String {
    let mut prompt = String::new();

    prompt.push_str(&format!(
        r#"You previously created a commit plan, but hunk {} was assigned to multiple commits.
Please choose which single commit should contain this hunk.

## The Conflicting Hunk

"#,
        hunk_id
    ));

    // Show the hunk
    if let Some(hunk) = context.hunks.iter().find(|h| h.id == hunk_id) {
        prompt.push_str(&format!("### Hunk {} - {}\n", hunk.id, hunk.file_path));
        prompt.push_str(&format!("```diff\n{}\n```\n\n", hunk.diff_content));
    }

    prompt.push_str("## Commits That Claim This Hunk\n\n");

    for &idx in commit_indices {
        if let Some(commit) = commits.get(idx) {
            prompt.push_str(&format!(
                "### Commit {} - \"{}\"\n{}\n\n",
                idx, commit.description.short, commit.description.long
            ));
        }
    }

    prompt.push_str(&format!(
        r#"## Your Task

Choose which commit should own hunk {}. Output a JSON object:

```json
{{"hunk_id": {}, "chosen_commit_index": N}}
```

Where N is the index (0-based) of the commit that should contain this hunk.
Output ONLY the JSON.

```json
"#,
        hunk_id, hunk_id
    ));

    prompt
}

/// Build a prompt to resolve overlapping hunk assignments
///
/// When two hunks have overlapping line ranges in the same file but are assigned
/// to different commits, we need to move them to the same commit to avoid conflicts.
pub fn build_fix_overlapping_prompt(
    context: &LlmContext,
    commits: &[crate::models::PlannedCommit],
    hunk_a_id: usize,
    commit_a_idx: usize,
    hunk_b_id: usize,
    commit_b_idx: usize,
    file_path: &std::path::Path,
) -> String {
    let mut prompt = String::new();

    prompt.push_str(&format!(
        r#"You previously created a commit plan, but hunks {} and {} have OVERLAPPING line ranges
in the same file ({}) and were assigned to different commits.

Overlapping hunks in different commits will cause conflicts when applied sequentially.
Both hunks must be in the SAME commit to be applied together correctly.

## The Overlapping Hunks

"#,
        hunk_a_id,
        hunk_b_id,
        file_path.display()
    ));

    // Show hunk A
    if let Some(hunk) = context.hunks.iter().find(|h| h.id == hunk_a_id) {
        prompt.push_str(&format!(
            "### Hunk {} (currently in Commit {})\n",
            hunk.id, commit_a_idx
        ));
        prompt.push_str(&format!(
            "Lines: old @{} (count: {})\n```diff\n{}\n```\n\n",
            hunk.old_start,
            context
                .hunks
                .iter()
                .find(|h| h.id == hunk_a_id)
                .map(|_| "see diff")
                .unwrap_or("?"),
            hunk.diff_content
        ));
    }

    // Show hunk B
    if let Some(hunk) = context.hunks.iter().find(|h| h.id == hunk_b_id) {
        prompt.push_str(&format!(
            "### Hunk {} (currently in Commit {})\n",
            hunk.id, commit_b_idx
        ));
        prompt.push_str(&format!(
            "Lines: old @{}\n```diff\n{}\n```\n\n",
            hunk.old_start, hunk.diff_content
        ));
    }

    prompt.push_str("## The Commits\n\n");

    // Show commit A
    if let Some(commit) = commits.get(commit_a_idx) {
        prompt.push_str(&format!(
            "### Commit {} - \"{}\"\n{}\n\n",
            commit_a_idx, commit.description.short, commit.description.long
        ));
    }

    // Show commit B
    if let Some(commit) = commits.get(commit_b_idx) {
        prompt.push_str(&format!(
            "### Commit {} - \"{}\"\n{}\n\n",
            commit_b_idx, commit.description.short, commit.description.long
        ));
    }

    prompt.push_str(&format!(
        r#"## Your Task

Choose which commit should contain BOTH overlapping hunks ({} and {}).
Consider which commit's purpose better matches both changes.

Output a JSON object:

```json
{{"hunk_a": {}, "hunk_b": {}, "chosen_commit_index": N}}
```

Where N is the index (0-based) of the commit that should contain both hunks.
Output ONLY the JSON.

```json
"#,
        hunk_a_id, hunk_b_id, hunk_a_id, hunk_b_id
    ));

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DiffLine;
    use crate::test_utils::{make_hunk_full, make_source_commit};

    #[test]
    fn test_build_context() {
        let commits = vec![make_source_commit("abc123", "Test commit")];

        let hunks = vec![make_hunk_full(
            0,
            "src/main.rs",
            vec![
                DiffLine::Context("fn main() {".to_string()),
                DiffLine::Added("    println!(\"Hello\");".to_string()),
                DiffLine::Context("}".to_string()),
            ],
            vec!["abc123".to_string()],
        )];

        let context = build_context(&commits, &hunks);
        assert_eq!(context.source_commits.len(), 1);
        assert_eq!(context.hunks.len(), 1);
        assert_eq!(context.hunks[0].id, 0);
        assert!(context.hunks[0].diff_content.contains("+    println!"));
    }
}
