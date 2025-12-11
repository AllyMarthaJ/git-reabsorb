//! Atomicity criterion: measures whether a commit represents a single, indivisible change.

use crate::assessment::criteria::{CriterionDefinition, CriterionId};
use crate::assessment::types::AssessmentLevel;

/// Returns the atomicity criterion definition.
pub fn definition() -> CriterionDefinition {
    CriterionDefinition {
        id: CriterionId::Atomicity,
        name: "Atomicity".to_string(),
        description: "Measures whether a commit represents a single, indivisible logical change. \
            An atomic commit can be understood, reviewed, and reverted as a single unit."
            .to_string(),
        levels: [
            AssessmentLevel::new(1, 1.0, "Multiple unrelated changes mixed together")
                .with_indicators(vec![
                    "Changes span completely unrelated subsystems".to_string(),
                    "Multiple distinct features or fixes in one commit".to_string(),
                    "No clear single purpose to the commit".to_string(),
                    "Would require multiple separate review discussions".to_string(),
                ]),
            AssessmentLevel::new(2, 1.0, "Multiple related but separable changes").with_indicators(
                vec![
                    "Changes are related but could be split into 2+ commits".to_string(),
                    "Contains both implementation and separate refactoring".to_string(),
                    "Includes drive-by fixes that could stand alone".to_string(),
                ],
            ),
            AssessmentLevel::new(3, 1.0, "Single logical change with minor extras")
                .with_indicators(vec![
                    "Core change is clear with small additions".to_string(),
                    "Minor formatting or cleanup mixed in".to_string(),
                    "Mostly atomic but not perfectly focused".to_string(),
                ]),
            AssessmentLevel::new(4, 1.0, "Single logical change, well-scoped").with_indicators(
                vec![
                    "Clear single purpose that all changes serve".to_string(),
                    "Changes are minimal for the stated goal".to_string(),
                    "Easy to review as a coherent unit".to_string(),
                ],
            ),
            AssessmentLevel::new(5, 1.0, "Perfect atomic unit, self-contained").with_indicators(
                vec![
                    "Cannot be meaningfully split further".to_string(),
                    "Every line serves the single stated purpose".to_string(),
                    "Ideal unit for review, revert, or cherry-pick".to_string(),
                    "Exemplary atomic commit".to_string(),
                ],
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_five_levels() {
        let def = definition();
        assert_eq!(def.levels.len(), 5);
        for (i, level) in def.levels.iter().enumerate() {
            assert_eq!(level.score, (i + 1) as u8);
        }
    }

    #[test]
    fn has_indicators() {
        let def = definition();
        for level in &def.levels {
            assert!(
                !level.indicators.is_empty(),
                "Level {} has no indicators",
                level.score
            );
        }
    }
}
