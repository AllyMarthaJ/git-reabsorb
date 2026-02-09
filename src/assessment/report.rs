//! Report formatting for assessment output.

use crate::assessment::criteria::get_definition;
use crate::assessment::types::{AssessmentComparison, CommitAssessment, RangeAssessment};

/// Output format for assessment reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable formatted output.
    Pretty,
    /// JSON output.
    Json,
    /// Markdown report.
    Markdown,
    /// Compact single-line per commit.
    Compact,
}

/// Format a range assessment for output.
pub fn format_assessment(
    assessment: &RangeAssessment,
    format: OutputFormat,
    verbose: bool,
) -> String {
    match format {
        OutputFormat::Pretty => format_pretty(assessment, verbose),
        OutputFormat::Json => format_json(assessment),
        OutputFormat::Markdown => format_markdown(assessment, verbose),
        OutputFormat::Compact => format_compact(assessment),
    }
}

/// Format a comparison for output.
pub fn format_comparison(comparison: &AssessmentComparison, format: OutputFormat) -> String {
    match format {
        OutputFormat::Pretty => format_comparison_pretty(comparison),
        OutputFormat::Json => format_comparison_json(comparison),
        OutputFormat::Markdown => format_comparison_markdown(comparison),
        OutputFormat::Compact => format_comparison_compact(comparison),
    }
}

fn format_pretty(assessment: &RangeAssessment, verbose: bool) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "Assessment: {}..{}\n",
        &assessment.base_sha[..8.min(assessment.base_sha.len())],
        &assessment.head_sha[..8.min(assessment.head_sha.len())]
    ));
    output.push_str(&format!(
        "Overall Score: {:.1}%\n\n",
        assessment.overall_score * 100.0
    ));

    // Aggregate scores
    output.push_str("Aggregate Scores:\n");
    let mut sorted_aggs: Vec<_> = assessment.aggregate_scores.values().collect();
    sorted_aggs.sort_by(|a, b| a.criterion_id.name().cmp(b.criterion_id.name()));

    for agg in sorted_aggs {
        output.push_str(&format!(
            "  {}: {:.1} (min: {:.0}, max: {:.0}, std: {:.2})\n",
            agg.criterion_id.name(),
            agg.mean_score,
            agg.min_score,
            agg.max_score,
            agg.std_deviation
        ));
    }
    output.push('\n');

    // Per-commit details
    output.push_str("Commits:\n");
    for commit in &assessment.commit_assessments {
        output.push_str(&format_commit_pretty(commit, verbose));
    }

    output
}

fn format_commit_pretty(commit: &CommitAssessment, verbose: bool) -> String {
    let mut output = String::new();
    let sha = &commit.commit_sha[..8.min(commit.commit_sha.len())];

    output.push_str(&format!(
        "\n{} {} ({:.1}%)\n\n",
        sha,
        commit.commit_message,
        commit.overall_score * 100.0
    ));

    // Format each criterion as a visual rubric
    for score in &commit.criterion_scores {
        output.push_str(&format_criterion_rubric(score, verbose));
        output.push('\n');
    }

    output
}

/// Format a single criterion score as a visual rubric table.
fn format_criterion_rubric(
    score: &crate::assessment::types::CriterionScore,
    verbose: bool,
) -> String {
    let mut output = String::new();

    // Get the criterion definition to access level descriptions
    let definition = get_definition(score.criterion_id);
    let name = score.criterion_id.name();

    // Column width for level descriptions
    let col_width = 24;

    // Criterion name header
    output.push_str(&format!("\x1b[1m{}\x1b[0m (Level {})\n", name, score.level));

    // Top border
    output.push('┌');
    for i in 0..5 {
        output.push_str(&"─".repeat(col_width));
        if i < 4 {
            output.push('┬');
        }
    }
    output.push_str("┐\n");

    // Level numbers row
    output.push('│');
    for i in 1..=5 {
        let is_hit = i == score.level;
        if is_hit {
            output.push_str(&format!(
                "\x1b[42;30m{:^width$}\x1b[0m",
                i,
                width = col_width
            ));
        } else {
            output.push_str(&format!("{:^width$}", i, width = col_width));
        }
        output.push('│');
    }
    output.push('\n');

    // Separator
    output.push('├');
    for i in 0..5 {
        output.push_str(&"─".repeat(col_width));
        if i < 4 {
            output.push('┼');
        }
    }
    output.push_str("┤\n");

    // Level descriptions - wrap text into multiple rows if needed
    let wrapped: Vec<Vec<String>> = definition
        .levels
        .iter()
        .map(|level| wrap_text(&level.description, col_width - 2))
        .collect();

    let max_lines = wrapped.iter().map(|w| w.len()).max().unwrap_or(1);

    for line_idx in 0..max_lines {
        output.push('│');
        for (col, lines) in wrapped.iter().enumerate() {
            let is_hit = (col + 1) as u8 == score.level;
            let text = lines.get(line_idx).map(|s| s.as_str()).unwrap_or("");

            if is_hit {
                output.push_str(&format!(
                    "\x1b[42;30m {:^width$} \x1b[0m",
                    text,
                    width = col_width - 2
                ));
            } else {
                output.push_str(&format!(" {:^width$} ", text, width = col_width - 2));
            }
            output.push('│');
        }
        output.push('\n');
    }

    // Bottom border
    output.push('└');
    for i in 0..5 {
        output.push_str(&"─".repeat(col_width));
        if i < 4 {
            output.push('┴');
        }
    }
    output.push_str("┘\n");

    // Verbose mode: show rationale, evidence, suggestions
    if verbose {
        output.push_str(&format!("  Rationale: {}\n", score.rationale));
        if !score.evidence.is_empty() {
            output.push_str("  Evidence:\n");
            for e in &score.evidence {
                output.push_str(&format!("    - {}\n", e));
            }
        }
        if !score.suggestions.is_empty() {
            output.push_str("  Suggestions:\n");
            for s in &score.suggestions {
                output.push_str(&format!("    - {}\n", s));
            }
        }
    }

    output
}

/// Wrap text to fit within a given width
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        if current_line.is_empty() {
            if word.len() > width {
                // Word is too long, truncate
                lines.push(word[..width].to_string());
            } else {
                current_line = word.to_string();
            }
        } else if current_line.len() + 1 + word.len() <= width {
            current_line.push(' ');
            current_line.push_str(word);
        } else {
            lines.push(current_line);
            current_line = word.to_string();
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn format_json(assessment: &RangeAssessment) -> String {
    serde_json::to_string_pretty(assessment).unwrap_or_else(|e| format!("Error: {}", e))
}

fn format_markdown(assessment: &RangeAssessment, verbose: bool) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "# Commit Assessment Report\n\n**Range**: `{}..{}`\n**Overall Score**: {:.1}%\n\n",
        &assessment.base_sha[..8.min(assessment.base_sha.len())],
        &assessment.head_sha[..8.min(assessment.head_sha.len())],
        assessment.overall_score * 100.0
    ));

    // Aggregate table
    output.push_str("## Summary\n\n| Criterion | Mean | Min | Max | Std Dev |\n|-----------|------|-----|-----|--------|\n");

    let mut sorted_aggs: Vec<_> = assessment.aggregate_scores.values().collect();
    sorted_aggs.sort_by(|a, b| a.criterion_id.name().cmp(b.criterion_id.name()));

    for agg in sorted_aggs {
        output.push_str(&format!(
            "| {} | {:.1} | {:.0} | {:.0} | {:.2} |\n",
            agg.criterion_id.name(),
            agg.mean_score,
            agg.min_score,
            agg.max_score,
            agg.std_deviation
        ));
    }
    output.push('\n');

    // Per-commit details
    output.push_str("## Commits\n\n");
    for commit in &assessment.commit_assessments {
        let sha = &commit.commit_sha[..8.min(commit.commit_sha.len())];
        output.push_str(&format!(
            "### `{}` {}\n\n**Score**: {:.1}%\n\n",
            sha,
            commit.commit_message,
            commit.overall_score * 100.0
        ));

        output.push_str("| Criterion | Level |\n|-----------|-------|\n");
        for score in &commit.criterion_scores {
            output.push_str(&format!("| {} | {} |\n", score.criterion_id, score.level));
        }
        output.push('\n');

        if verbose {
            for score in &commit.criterion_scores {
                output.push_str(&format!(
                    "**{}**: {}\n\n",
                    score.criterion_id, score.rationale
                ));
            }
        }
    }

    output
}

fn format_compact(assessment: &RangeAssessment) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "Overall: {:.1}%\n",
        assessment.overall_score * 100.0
    ));

    for commit in &assessment.commit_assessments {
        let sha = &commit.commit_sha[..8.min(commit.commit_sha.len())];
        let scores: Vec<String> = commit
            .criterion_scores
            .iter()
            .map(|s| {
                let id_str = s.criterion_id.to_string();
                format!("{}:{}", &id_str[..3.min(id_str.len())], s.level)
            })
            .collect();
        output.push_str(&format!(
            "{} {:.0}% [{}] {}\n",
            sha,
            commit.overall_score * 100.0,
            scores.join(" "),
            commit.commit_message
        ));
    }

    output
}

fn format_comparison_pretty(comparison: &AssessmentComparison) -> String {
    let mut output = String::new();

    let direction = if comparison.overall_delta > 0.0 {
        "+"
    } else {
        ""
    };
    output.push_str(&format!(
        "Assessment Comparison\n\nBefore: {:.1}% -> After: {:.1}% ({}{:.1}%)\n\n",
        comparison.before.overall_score * 100.0,
        comparison.after.overall_score * 100.0,
        direction,
        comparison.overall_delta * 100.0
    ));

    if !comparison.improvements.is_empty() {
        output.push_str("Improvements:\n");
        for imp in &comparison.improvements {
            output.push_str(&format!("  + {}\n", imp));
        }
        output.push('\n');
    }

    if !comparison.regressions.is_empty() {
        output.push_str("Regressions:\n");
        for reg in &comparison.regressions {
            output.push_str(&format!("  - {}\n", reg));
        }
        output.push('\n');
    }

    output
}

fn format_comparison_json(comparison: &AssessmentComparison) -> String {
    serde_json::to_string_pretty(comparison).unwrap_or_else(|e| format!("Error: {}", e))
}

fn format_comparison_markdown(comparison: &AssessmentComparison) -> String {
    let mut output = String::new();

    let direction = if comparison.overall_delta > 0.0 {
        "+"
    } else {
        ""
    };
    output.push_str(&format!(
        "# Assessment Comparison\n\n**Before**: {:.1}%\n**After**: {:.1}%\n**Change**: {}{:.1}%\n\n",
        comparison.before.overall_score * 100.0,
        comparison.after.overall_score * 100.0,
        direction,
        comparison.overall_delta * 100.0
    ));

    if !comparison.improvements.is_empty() {
        output.push_str("## Improvements\n\n");
        for imp in &comparison.improvements {
            output.push_str(&format!("- {}\n", imp));
        }
        output.push('\n');
    }

    if !comparison.regressions.is_empty() {
        output.push_str("## Regressions\n\n");
        for reg in &comparison.regressions {
            output.push_str(&format!("- {}\n", reg));
        }
    }

    output
}

fn format_comparison_compact(comparison: &AssessmentComparison) -> String {
    let direction = if comparison.overall_delta > 0.0 {
        "+"
    } else {
        ""
    };
    format!(
        "{:.1}% -> {:.1}% ({}{:.1}%)",
        comparison.before.overall_score * 100.0,
        comparison.after.overall_score * 100.0,
        direction,
        comparison.overall_delta * 100.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assessment::criteria::CriterionId;
    use crate::assessment::types::{CriterionScore, RangeAssessment};
    use std::collections::HashMap;

    fn make_test_assessment() -> RangeAssessment {
        RangeAssessment {
            base_sha: "abc12345".to_string(),
            head_sha: "def67890".to_string(),
            assessed_at: "2024-01-01T00:00:00Z".to_string(),
            commit_assessments: vec![CommitAssessment {
                commit_sha: "abc12345".to_string(),
                commit_message: "Test commit".to_string(),
                criterion_scores: vec![CriterionScore {
                    criterion_id: CriterionId::Atomicity,
                    level: 4,
                    weighted_score: 4.0,
                    rationale: "Good".to_string(),
                    evidence: vec!["Single change".to_string()],
                    suggestions: vec![],
                }],
                overall_score: 0.8,
                position: 0,
                total_commits: 1,
            }],
            aggregate_scores: HashMap::new(),
            overall_score: 0.8,
            range_observations: vec![],
        }
    }

    #[test]
    fn pretty_format_includes_score() {
        let assessment = make_test_assessment();
        let output = format_assessment(&assessment, OutputFormat::Pretty, false);
        assert!(output.contains("80.0%"));
    }

    #[test]
    fn compact_format_is_brief() {
        let assessment = make_test_assessment();
        let output = format_assessment(&assessment, OutputFormat::Compact, false);
        assert!(output.lines().count() <= 3);
    }

    #[test]
    fn json_format_is_valid() {
        let assessment = make_test_assessment();
        let output = format_assessment(&assessment, OutputFormat::Json, false);
        let parsed: Result<RangeAssessment, _> = serde_json::from_str(&output);
        assert!(parsed.is_ok());
    }
}
