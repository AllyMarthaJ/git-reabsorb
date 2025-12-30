//! Message quality criterion: measures how well the commit message communicates intent.

use crate::assessment::criteria::{CriterionDefinition, CriterionId};
use crate::assessment::types::AssessmentLevel;

/// Returns the message quality criterion definition.
///
/// This criterion has a higher weight (1.2) because commit messages are
/// critical for long-term code maintenance and understanding.
pub fn definition() -> CriterionDefinition {
    CriterionDefinition {
        id: CriterionId::MessageQuality,
        description: "Measures how well the commit message describes the motivation behind \
            the change, why it was required, and what implications it has. Good commit \
            messages explain the 'why', not just the 'what'."
            .to_string(),
        levels: [
            AssessmentLevel::new(1, 1.2, "Missing or meaningless message").with_indicators(vec![
                "Single word like 'fix', 'update', 'changes'".to_string(),
                "No message body at all".to_string(),
                "WIP or temporary message".to_string(),
                "Completely uninformative".to_string(),
            ]),
            AssessmentLevel::new(2, 1.2, "Describes what, not why").with_indicators(vec![
                "Lists files or functions changed".to_string(),
                "Says what was done but not the reason".to_string(),
                "No context for future readers".to_string(),
                "Example: 'Update auth.rs'".to_string(),
            ]),
            AssessmentLevel::new(3, 1.2, "Adequate context, some motivation").with_indicators(
                vec![
                    "Subject line explains the change".to_string(),
                    "Some context in body".to_string(),
                    "Partially explains why".to_string(),
                    "Could be clearer but acceptable".to_string(),
                ],
            ),
            AssessmentLevel::new(4, 1.2, "Clear motivation and implications").with_indicators(
                vec![
                    "Subject line is clear and imperative".to_string(),
                    "Body explains the motivation".to_string(),
                    "Mentions implications or side effects".to_string(),
                    "Future reader would understand context".to_string(),
                ],
            ),
            AssessmentLevel::new(5, 1.2, "Excellent: why, what, implications, alternatives")
                .with_indicators(vec![
                    "Perfect subject line (50 chars, imperative, explains why)".to_string(),
                    "Body thoroughly explains motivation".to_string(),
                    "Discusses alternatives considered".to_string(),
                    "References issues/context appropriately".to_string(),
                    "Model commit message".to_string(),
                ]),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_higher_weight() {
        let def = definition();
        assert_eq!(def.levels[0].weight, 1.2);
    }

    #[test]
    fn max_weighted_score() {
        let def = definition();
        assert_eq!(def.max_weighted_score(), 6.0); // 5 * 1.2
    }
}
