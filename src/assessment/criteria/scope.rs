//! Scope appropriateness criterion: measures whether the commit's size is appropriate.

use crate::assessment::criteria::{CriterionDefinition, CriterionId};
use crate::assessment::types::AssessmentLevel;

/// Returns the scope appropriateness criterion definition.
///
/// This criterion has a lower weight (0.8) because scope is somewhat
/// subjective and context-dependent.
pub fn definition() -> CriterionDefinition {
    CriterionDefinition {
        id: CriterionId::ScopeAppropriateness,
        description: "Measures whether the commit's size and scope are appropriate for \
            effective code review and understanding. Too large commits are hard to review; \
            too small commits add noise to history."
            .to_string(),
        levels: [
            AssessmentLevel::new(1, 0.8, "Massive dump or trivial whitespace-only")
                .with_indicators(vec![
                    "Hundreds of lines changed across many files".to_string(),
                    "Only whitespace or formatting changes".to_string(),
                    "Impossible to review effectively".to_string(),
                    "History pollution with no value".to_string(),
                ]),
            AssessmentLevel::new(2, 0.8, "Too large to review effectively").with_indicators(vec![
                "More than 400 lines changed".to_string(),
                "Touches many unrelated files".to_string(),
                "Would take significant time to review".to_string(),
                "Should probably be split".to_string(),
            ]),
            AssessmentLevel::new(3, 0.8, "Reviewable but could be split").with_indicators(vec![
                "200-400 lines changed".to_string(),
                "Multiple logical sections visible".to_string(),
                "Reviewer could handle it but not ideal".to_string(),
            ]),
            AssessmentLevel::new(4, 0.8, "Good size for review").with_indicators(vec![
                "50-200 lines changed".to_string(),
                "Comfortable to review in one session".to_string(),
                "Scope matches complexity well".to_string(),
            ]),
            AssessmentLevel::new(5, 0.8, "Ideal size: meaningful but digestible").with_indicators(
                vec![
                    "Optimal balance of content and reviewability".to_string(),
                    "Can be understood in minutes".to_string(),
                    "Size is proportional to conceptual change".to_string(),
                    "Neither too big nor too small".to_string(),
                ],
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_lower_weight() {
        let def = definition();
        assert_eq!(def.levels[0].weight, 0.8);
    }

    #[test]
    fn max_weighted_score() {
        let def = definition();
        assert_eq!(def.max_weighted_score(), 4.0); // 5 * 0.8
    }
}
