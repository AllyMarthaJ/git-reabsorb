//! Reversibility criterion: measures how easily the commit could be reverted.

use crate::assessment::criteria::{CriterionDefinition, CriterionId};
use crate::assessment::types::AssessmentLevel;

/// Returns the reversibility criterion definition.
///
/// This criterion has a lower weight (0.8) because reversibility depends
/// heavily on context and the nature of the changes.
pub fn definition() -> CriterionDefinition {
    CriterionDefinition {
        id: CriterionId::Reversibility,
        name: "Reversibility".to_string(),
        description: "Measures how easily the commit could be reverted if needed. \
            Good commits are isolated enough that reverting them doesn't cascade \
            into other changes or require manual conflict resolution."
            .to_string(),
        levels: [
            AssessmentLevel::new(1, 0.8, "Cannot revert without breaking other commits")
                .with_indicators(vec![
                    "Tightly coupled to subsequent commits".to_string(),
                    "Partial feature that other commits depend on".to_string(),
                    "Reverting would leave code in broken state".to_string(),
                    "Contains irreversible operations (e.g., data deletion)".to_string(),
                ]),
            AssessmentLevel::new(2, 0.8, "Revert requires manual conflict resolution")
                .with_indicators(vec![
                    "Would conflict with later changes".to_string(),
                    "Requires understanding of subsequent commits".to_string(),
                    "Non-trivial merge resolution needed".to_string(),
                ]),
            AssessmentLevel::new(3, 0.8, "Can revert with some cascading effects").with_indicators(
                vec![
                    "Clean revert possible but affects later commits".to_string(),
                    "May need follow-up changes after revert".to_string(),
                    "Some dependencies exist but manageable".to_string(),
                ],
            ),
            AssessmentLevel::new(4, 0.8, "Clean revert with minimal impact").with_indicators(vec![
                "Git revert works cleanly".to_string(),
                "Minimal impact on other changes".to_string(),
                "Self-contained enough for safe revert".to_string(),
            ]),
            AssessmentLevel::new(5, 0.8, "Perfectly reversible, isolated impact").with_indicators(
                vec![
                    "Completely independent of other commits".to_string(),
                    "No external dependencies created".to_string(),
                    "Could be reverted at any point in history".to_string(),
                    "Ideal for cherry-picking or reverting".to_string(),
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
    fn has_five_levels() {
        let def = definition();
        assert_eq!(def.levels.len(), 5);
        for (i, level) in def.levels.iter().enumerate() {
            assert_eq!(level.score, (i + 1) as u8);
        }
    }
}
