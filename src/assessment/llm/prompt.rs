//! Prompt construction for LLM-based commit assessment.

use crate::assessment::criteria::{CriterionDefinition, RangeContext};
use crate::models::SourceCommit;

/// Builds an assessment prompt for a single criterion.
pub fn build_assessment_prompt(
    definition: &CriterionDefinition,
    commit: &SourceCommit,
    diff_content: &str,
    range_context: &RangeContext,
) -> String {
    let mut prompt = String::new();

    // Header and criterion description
    prompt.push_str(&format!(
        r#"You are assessing a git commit for "{}" quality.

## Criterion: {}

{}

## Rubric

"#,
        definition.id.name(),
        definition.id.name(),
        definition.description
    ));

    // Add rubric levels
    for level in &definition.levels {
        prompt.push_str(&format!(
            "### Level {} (weight: {:.1})\n{}\n\nIndicators:\n",
            level.score, level.weight, level.description
        ));
        for indicator in &level.indicators {
            prompt.push_str(&format!("- {}\n", indicator));
        }
        prompt.push('\n');
    }

    // Add commit context
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

"#,
        short_sha,
        commit.message.long,
        range_context.position + 1,
        range_context.commits.len(),
        truncate_diff(diff_content, 3000)
    ));

    // Add range context if multiple commits
    if range_context.commits.len() > 1 {
        prompt.push_str("## Other commits in range (for context)\n\n");
        for (i, c) in range_context.commits.iter().enumerate() {
            if i != range_context.position {
                let sha = &c.sha[..8.min(c.sha.len())];
                prompt.push_str(&format!("- {} {}\n", sha, c.message.short));
            }
        }
        prompt.push('\n');
    }

    // Add response format
    prompt.push_str(
        r#"## Your Task

Assess this commit against the rubric above and output JSON with:
1. `level`: Score 1-5 matching the rubric level that best describes this commit
2. `rationale`: Brief explanation for why you chose this level (1-2 sentences)
3. `evidence`: Array of specific observations from the diff/message supporting your assessment
4. `suggestions`: Array of improvement suggestions (empty array if level 5)

Output ONLY valid JSON, no markdown fences:

{"level": <1-5>, "rationale": "...", "evidence": ["...", "..."], "suggestions": ["...", "..."]}
"#,
    );

    prompt
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
    use crate::assessment::criteria::atomicity;
    use crate::models::SourceCommit;

    #[test]
    fn builds_complete_prompt() {
        let def = atomicity::definition();
        let commit = SourceCommit::new(
            "abc123def",
            "Add feature",
            "Add feature\n\nThis adds a new feature.",
        );
        let context = RangeContext::new(vec![commit.clone()], 0);
        let diff = "+fn new_function() {}";

        let prompt = build_assessment_prompt(&def, &commit, diff, &context);

        assert!(prompt.contains("Atomicity"));
        assert!(prompt.contains("abc123de"));
        assert!(prompt.contains("Add feature"));
        assert!(prompt.contains("+fn new_function"));
        assert!(prompt.contains("Level 1"));
        assert!(prompt.contains("Level 5"));
    }

    #[test]
    fn truncates_long_diff() {
        let long_diff = "x".repeat(5000);
        let truncated = truncate_diff(&long_diff, 100);
        assert!(truncated.len() <= 100);
    }
}
