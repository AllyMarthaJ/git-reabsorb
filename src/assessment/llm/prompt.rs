//! Prompt construction for LLM-based commit assessment.

use crate::assessment::criteria::{CriterionDefinition, DiffStats, RangeContext};
use crate::models::SourceCommit;

/// Builds a batched assessment prompt for all criteria at once.
pub fn build_assessment_prompt(
    definitions: &[CriterionDefinition],
    commit: &SourceCommit,
    diff_content: &str,
    diff_stats: &DiffStats,
    range_context: &RangeContext,
    max_context_commits: usize,
) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        "You are assessing a git commit for quality across multiple criteria.\n\n\
         Note: Criteria have different weights reflecting their relative importance.\n\
         Higher-weight criteria should be assessed more carefully.\n\n\
         ## Criteria\n\n",
    );

    // Compact rubric: one line per level
    for def in definitions {
        prompt.push_str(&format!(
            "### {} (weight: {:.1})\n\n{}\n\n",
            def.id.name(),
            def.levels[0].weight,
            def.description
        ));
        for level in &def.levels {
            prompt.push_str(&format!("- Level {}: {}\n", level.score, level.description));
        }
        prompt.push('\n');
    }

    // Commit context
    let short_sha = &commit.sha[..8.min(commit.sha.len())];
    prompt.push_str(&format!(
        r#"## Commit to Assess

**SHA**: {}
**Message**:
```
{}
```

**Position in range**: {} of {}

**Diff**:
```diff
{}
```

## Diff Statistics
- Lines added: {}
- Lines removed: {}
- Total lines changed: {}
- Files changed: {}

"#,
        short_sha,
        commit.message.long,
        range_context.position + 1,
        range_context.commits.len(),
        truncate_diff(diff_content, 3000),
        diff_stats.lines_added,
        diff_stats.lines_removed,
        diff_stats.total_lines(),
        diff_stats.files_changed,
    ));

    // Range context with capping
    if range_context.commits.len() > 1 {
        prompt.push_str("## Other commits in range (for context)\n\n");

        let selected = select_context_commits(
            &range_context.commits,
            range_context.position,
            max_context_commits,
        );

        for (i, c) in &selected {
            let sha = &c.sha[..8.min(c.sha.len())];
            prompt.push_str(&format!(
                "- [{}/{}] {} {}\n",
                i + 1,
                range_context.commits.len(),
                sha,
                c.message.short
            ));
        }

        let total_others = range_context.commits.len() - 1;
        if selected.len() < total_others {
            let first = &range_context.commits[0];
            let last = &range_context.commits[range_context.commits.len() - 1];
            let first_sha = &first.sha[..8.min(first.sha.len())];
            let last_sha = &last.sha[..8.min(last.sha.len())];
            prompt.push_str(&format!(
                "\n... and {} more commits not shown (range: {}..{})\n",
                total_others - selected.len(),
                first_sha,
                last_sha
            ));
        }
        prompt.push('\n');
    }

    // Build criterion ID list for the JSON example
    let criterion_examples: Vec<String> = definitions
        .iter()
        .map(|d| {
            format!(
                "    {{\"criterion\": \"{}\", \"level\": <1-5>, \"rationale\": \"...\", \"evidence\": [\"...\"], \"suggestions\": [\"...\"]}}",
                d.id
            )
        })
        .collect();

    prompt.push_str(&format!(
        r#"## Your Task

Assess this commit against ALL criteria above. Output a single JSON object:

{{
  "scores": [
{}
  ]
}}

Output ONLY valid JSON, no markdown fences.
"#,
        criterion_examples.join(",\n")
    ));

    prompt
}

/// Select up to `max` context commits nearest to `position`, excluding the commit at `position`.
///
/// Splits evenly before and after. If one side has fewer commits, the other side gets more.
/// Returns (original_index, commit) pairs.
fn select_context_commits(
    all_commits: &[SourceCommit],
    position: usize,
    max: usize,
) -> Vec<(usize, &SourceCommit)> {
    let total = all_commits.len();
    if total <= 1 {
        return Vec::new();
    }

    // All others excluding the commit at position
    let others: Vec<(usize, &SourceCommit)> = all_commits
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != position)
        .collect();

    if others.len() <= max {
        return others;
    }

    // Split into before and after
    let before: Vec<(usize, &SourceCommit)> = others
        .iter()
        .filter(|(i, _)| *i < position)
        .cloned()
        .collect();
    let after: Vec<(usize, &SourceCommit)> = others
        .iter()
        .filter(|(i, _)| *i > position)
        .cloned()
        .collect();

    let half = max / 2;
    let (take_before, take_after) = if before.len() < half {
        (before.len(), max - before.len())
    } else if after.len() < half {
        (max - after.len(), after.len())
    } else {
        (half, max - half)
    };

    // Take the closest `take_before` from before (end of the vec) and closest `take_after` from after (start of the vec)
    let mut result: Vec<(usize, &SourceCommit)> = Vec::with_capacity(max);
    let before_start = before.len().saturating_sub(take_before);
    result.extend_from_slice(&before[before_start..]);
    result.extend_from_slice(&after[..take_after.min(after.len())]);

    result
}

/// Truncate diff content to avoid exceeding token limits.
fn truncate_diff(diff: &str, max_chars: usize) -> &str {
    if diff.len() <= max_chars {
        diff
    } else {
        // Try to truncate at a line boundary
        let truncated = &diff[..max_chars];
        if let Some(last_newline) = truncated.rfind('\n') {
            &diff[..last_newline]
        } else {
            truncated
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assessment::criteria::coherence;
    use crate::models::SourceCommit;

    #[test]
    fn builds_complete_prompt() {
        let defs = vec![coherence::definition()];
        let commit = SourceCommit::new(
            "abc123def",
            "Add feature",
            "Add feature\n\nThis adds a new feature.",
        );
        let context = RangeContext::new(vec![commit.clone()], 0);
        let diff = "+fn new_function() {}";
        let stats = DiffStats {
            lines_added: 1,
            lines_removed: 0,
            files_changed: 1,
        };

        let prompt = build_assessment_prompt(&defs, &commit, diff, &stats, &context, 10);

        assert!(prompt.contains("Coherence"));
        assert!(prompt.contains("abc123de"));
        assert!(prompt.contains("Add feature"));
        assert!(prompt.contains("+fn new_function"));
        assert!(prompt.contains("Level 1"));
        assert!(prompt.contains("Level 5"));
        assert!(prompt.contains("\"criterion\": \"coherence\""));
        assert!(prompt.contains("Lines added: 1"));
        assert!(prompt.contains("Files changed: 1"));
        assert!(prompt.contains("Higher-weight criteria"));
    }

    #[test]
    fn truncates_long_diff() {
        let long_diff = "x".repeat(5000);
        let truncated = truncate_diff(&long_diff, 100);
        assert!(truncated.len() <= 100);
    }

    #[test]
    fn select_context_all_fit() {
        let commits: Vec<SourceCommit> = (0..5)
            .map(|i| SourceCommit::new(format!("sha{}", i), format!("Commit {}", i), ""))
            .collect();

        let selected = select_context_commits(&commits, 2, 10);
        assert_eq!(selected.len(), 4); // all except position 2
    }

    #[test]
    fn select_context_capped() {
        let commits: Vec<SourceCommit> = (0..20)
            .map(|i| SourceCommit::new(format!("sha{:02}", i), format!("Commit {}", i), ""))
            .collect();

        let selected = select_context_commits(&commits, 10, 6);
        assert_eq!(selected.len(), 6);
        // Should have 3 before and 3 after
        let before_count = selected.iter().filter(|(i, _)| *i < 10).count();
        let after_count = selected.iter().filter(|(i, _)| *i > 10).count();
        assert_eq!(before_count, 3);
        assert_eq!(after_count, 3);
    }

    #[test]
    fn select_context_near_start() {
        let commits: Vec<SourceCommit> = (0..20)
            .map(|i| SourceCommit::new(format!("sha{:02}", i), format!("Commit {}", i), ""))
            .collect();

        let selected = select_context_commits(&commits, 1, 6);
        assert_eq!(selected.len(), 6);
        // Only 1 before (position 0), so 5 after
        let before_count = selected.iter().filter(|(i, _)| *i < 1).count();
        let after_count = selected.iter().filter(|(i, _)| *i > 1).count();
        assert_eq!(before_count, 1);
        assert_eq!(after_count, 5);
    }

    #[test]
    fn select_context_near_end() {
        let commits: Vec<SourceCommit> = (0..20)
            .map(|i| SourceCommit::new(format!("sha{:02}", i), format!("Commit {}", i), ""))
            .collect();

        let selected = select_context_commits(&commits, 18, 6);
        assert_eq!(selected.len(), 6);
        // Only 1 after (position 19), so 5 before
        let before_count = selected.iter().filter(|(i, _)| *i < 18).count();
        let after_count = selected.iter().filter(|(i, _)| *i > 18).count();
        assert_eq!(before_count, 5);
        assert_eq!(after_count, 1);
    }

    #[test]
    fn prompt_caps_context_with_range_note() {
        let commits: Vec<SourceCommit> = (0..20)
            .map(|i| SourceCommit::new(format!("sha{:02}abc", i), format!("Commit {}", i), ""))
            .collect();
        let defs = vec![coherence::definition()];
        let context = RangeContext::new(commits, 10);
        let stats = DiffStats {
            lines_added: 5,
            lines_removed: 2,
            files_changed: 1,
        };

        let prompt = build_assessment_prompt(
            &defs,
            &context.commits[10].clone(),
            "+code",
            &stats,
            &context,
            4,
        );

        assert!(prompt.contains("more commits not shown"));
        assert!(prompt.contains("sha00abc")); // first sha in range note
        assert!(prompt.contains("sha19abc")); // last sha in range note
    }
}
