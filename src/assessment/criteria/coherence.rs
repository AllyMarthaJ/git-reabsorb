//! Coherence criterion: measures whether all changes form a single logical unit.
//!
//! Merges the former Atomicity and Logical Cohesion criteria into one, since both
//! asked "do these changes belong together?" and produced correlated scores.

use crate::assessment::criteria::{CriterionDefinition, CriterionId};
use crate::assessment::types::AssessmentLevel;

/// Returns the coherence criterion definition.
pub fn definition() -> CriterionDefinition {
    CriterionDefinition {
        id: CriterionId::Coherence,
        description: "Measures whether all changes in the commit form a single, coherent \
            logical unit. A coherent commit has one clear purpose, every change serves that \
            purpose, and it could be meaningfully reviewed as a self-contained unit."
            .to_string(),
        levels: [
            AssessmentLevel::new(1, 1.0, "Random assortment, no relationship between changes")
                .with_indicators(vec![
                    "Changes span completely unrelated subsystems".to_string(),
                    "Multiple distinct features or fixes in one commit".to_string(),
                    "No clear single purpose to the commit".to_string(),
                    "Appears to be multiple commits squashed together".to_string(),
                ]),
            AssessmentLevel::new(
                2,
                1.0,
                "Related but contains separable concerns or drive-by fixes",
            )
            .with_indicators(vec![
                "Changes are related but could be split into 2+ commits".to_string(),
                "Contains both implementation and separate refactoring".to_string(),
                "Includes drive-by fixes that could stand alone".to_string(),
                "Connected by timing rather than purpose".to_string(),
            ]),
            AssessmentLevel::new(3, 1.0, "Single logical change with minor unrelated extras")
                .with_indicators(vec![
                    "Core change is clear with small additions".to_string(),
                    "Minor formatting or cleanup mixed in".to_string(),
                    "Mostly coherent but not perfectly focused".to_string(),
                ]),
            AssessmentLevel::new(4, 1.0, "All changes serve a clear common purpose, well-scoped")
                .with_indicators(vec![
                    "Clear single purpose that all changes serve".to_string(),
                    "Changes are minimal for the stated goal".to_string(),
                    "Easy to review as a coherent unit".to_string(),
                    "No surprising inclusions".to_string(),
                ]),
            AssessmentLevel::new(
                5,
                1.0,
                "Perfect coherent unit — every line necessary, nothing extra",
            )
            .with_indicators(vec![
                "Cannot be meaningfully split further".to_string(),
                "Every line serves the single stated purpose".to_string(),
                "Ideal unit for review, revert, or cherry-pick".to_string(),
                "Nothing extra, nothing missing".to_string(),
            ]),
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
