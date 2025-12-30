//! Logical cohesion criterion: measures whether all changes belong together semantically.

use crate::assessment::criteria::{CriterionDefinition, CriterionId};
use crate::assessment::types::AssessmentLevel;

/// Returns the logical cohesion criterion definition.
pub fn definition() -> CriterionDefinition {
    CriterionDefinition {
        id: CriterionId::LogicalCohesion,
        description: "Measures whether all changes in the commit belong together semantically. \
            High cohesion means every change serves the same logical purpose and a reviewer \
            would naturally expect these changes together."
            .to_string(),
        levels: [
            AssessmentLevel::new(1, 1.0, "Random assortment of changes").with_indicators(vec![
                "No discernible relationship between changes".to_string(),
                "Appears to be multiple commits squashed together".to_string(),
                "Changes serve completely different purposes".to_string(),
            ]),
            AssessmentLevel::new(2, 1.0, "Some relationship but weak").with_indicators(vec![
                "Changes are loosely related".to_string(),
                "Connected by timing rather than purpose".to_string(),
                "Some changes feel out of place".to_string(),
            ]),
            AssessmentLevel::new(3, 1.0, "Related by proximity/timing, not purpose")
                .with_indicators(vec![
                    "Changes are in nearby code".to_string(),
                    "Made at the same time but different goals".to_string(),
                    "Would benefit from separation".to_string(),
                ]),
            AssessmentLevel::new(4, 1.0, "All changes serve a common purpose").with_indicators(
                vec![
                    "Clear single goal that all changes support".to_string(),
                    "Reviewer understands why these are together".to_string(),
                    "No surprising inclusions".to_string(),
                ],
            ),
            AssessmentLevel::new(5, 1.0, "Changes form a coherent, minimal unit").with_indicators(
                vec![
                    "Every change is necessary for the goal".to_string(),
                    "Nothing extra, nothing missing".to_string(),
                    "Perfect logical grouping".to_string(),
                    "Natural unit of work".to_string(),
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
    }
}
