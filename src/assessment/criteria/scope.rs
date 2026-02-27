//! Scope appropriateness criterion: measures whether the commit's size is appropriate.
//!
//! This criterion is scored deterministically from diff statistics rather than
//! by the LLM, since the LLM cannot reliably count lines in a truncated diff.

use crate::assessment::criteria::{CriterionDefinition, CriterionId};
use crate::assessment::types::AssessmentLevel;

/// Returns the scope appropriateness criterion definition.
///
/// This criterion has a lower weight (0.8) because scope is somewhat
/// context-dependent. It is scored deterministically by `compute_scope_score()`
/// rather than by the LLM.
pub fn definition() -> CriterionDefinition {
    CriterionDefinition {
        id: CriterionId::ScopeAppropriateness,
        description: "Measures whether the commit's size and scope are appropriate for \
            effective code review and understanding. Scored deterministically from diff \
            statistics (lines added/removed, files changed)."
            .to_string(),
        levels: [
            AssessmentLevel::new(1, 0.8, "Massive dump (>800 lines) or empty/whitespace-only"),
            AssessmentLevel::new(2, 0.8, "Too large (>400 lines) or too many files for size"),
            AssessmentLevel::new(3, 0.8, "Reviewable but could be split (200-400 lines)"),
            AssessmentLevel::new(4, 0.8, "Good size for review (30-200 lines)"),
            AssessmentLevel::new(5, 0.8, "Ideal: focused and digestible (1-30 lines)"),
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
