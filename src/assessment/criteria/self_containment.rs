//! Self-containment criterion: measures whether the commit includes everything
//! needed for its stated purpose.
//!
//! Replaces the former Reversibility criterion, which required commit graph
//! knowledge the LLM cannot observe from a diff alone. Self-containment is
//! assessable from visible signals: missing imports, dangling references,
//! incomplete interface implementations, etc.

use crate::assessment::criteria::{CriterionDefinition, CriterionId};
use crate::assessment::types::AssessmentLevel;

/// Returns the self-containment criterion definition.
///
/// This criterion has a lower weight (0.8) because completeness is
/// context-dependent and harder to assess perfectly from a diff alone.
pub fn definition() -> CriterionDefinition {
    CriterionDefinition {
        id: CriterionId::SelfContainment,
        description: "Measures whether the commit includes everything needed for its stated \
            purpose. A self-contained commit doesn't leave dangling references, partial \
            implementations, or require follow-up commits to be functional."
            .to_string(),
        levels: [
            AssessmentLevel::new(
                1,
                0.8,
                "Partial feature, dangling references, broken without follow-up",
            )
            .with_indicators(vec![
                "References functions or types not defined in this commit or existing code"
                    .to_string(),
                "Adds imports that are never used, or uses symbols not imported".to_string(),
                "Commit alone would leave the build broken".to_string(),
                "Clearly the first half of a two-part change".to_string(),
            ]),
            AssessmentLevel::new(
                2,
                0.8,
                "Mostly complete but leaves loose ends (unused imports, stubs)",
            )
            .with_indicators(vec![
                "Stub implementations or TODO placeholders left behind".to_string(),
                "Unused imports or dead code introduced".to_string(),
                "Feature is partially wired up but not fully connected".to_string(),
                "Would compile but has obvious incomplete pieces".to_string(),
            ]),
            AssessmentLevel::new(
                3,
                0.8,
                "Functional but missing related updates (docs, tests, config)",
            )
            .with_indicators(vec![
                "Core change works but related docs not updated".to_string(),
                "No tests for new functionality".to_string(),
                "Config or schema changes not included".to_string(),
                "Works in isolation but not production-ready".to_string(),
            ]),
            AssessmentLevel::new(
                4,
                0.8,
                "Complete change — all necessary files updated, no dangling references",
            )
            .with_indicators(vec![
                "All referenced symbols exist or are properly imported".to_string(),
                "Interface changes reflected in implementations".to_string(),
                "No obvious loose ends or TODOs".to_string(),
                "Related files updated appropriately".to_string(),
            ]),
            AssessmentLevel::new(
                5,
                0.8,
                "Fully self-contained — works in isolation, includes tests/docs if applicable",
            )
            .with_indicators(vec![
                "Includes tests for new or changed behavior".to_string(),
                "Documentation updated where relevant".to_string(),
                "Could be cherry-picked to any branch and work".to_string(),
                "Exemplary completeness for the scope of the change".to_string(),
            ]),
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
