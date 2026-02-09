//! Response parsing for LLM-based commit assessment.

use std::collections::HashSet;

use serde::Deserialize;

use crate::assessment::criteria::{CriterionDefinition, CriterionId};
use crate::assessment::types::CriterionScore;
use crate::utils::extract_json_str;

/// Batched LLM response containing scores for all criteria.
#[derive(Debug, Deserialize)]
struct LlmBatchedResponse {
    scores: Vec<LlmBatchedScore>,
}

/// A single criterion score within the batched response.
#[derive(Debug, Deserialize)]
struct LlmBatchedScore {
    criterion: String,
    level: u8,
    rationale: String,
    evidence: Vec<String>,
    #[serde(default)]
    suggestions: Vec<String>,
}

/// Parse error types.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("No JSON found in response")]
    NoJson,
    #[error("Invalid JSON: {0}")]
    InvalidJson(String),
    #[error("Level {0} out of range 1-5")]
    LevelOutOfRange(u8),
    #[error("Missing criteria in response: {0}")]
    MissingCriteria(String),
    #[error("Unknown criterion: {0}")]
    UnknownCriterion(String),
    #[error("Duplicate criterion in response: {0}")]
    DuplicateCriterion(String),
}

/// Parse a batched LLM response into criterion scores.
///
/// Expects a JSON object with a `scores` array, where each element has a `criterion`
/// string matching a `CriterionId`, plus `level`, `rationale`, `evidence`, and `suggestions`.
pub fn parse_assessment_response(
    response: &str,
    definitions: &[CriterionDefinition],
) -> Result<Vec<CriterionScore>, ParseError> {
    let json_str = extract_json_str(response).ok_or(ParseError::NoJson)?;

    let parsed: LlmBatchedResponse =
        serde_json::from_str(json_str).map_err(|e| ParseError::InvalidJson(e.to_string()))?;

    let mut scores = Vec::with_capacity(definitions.len());
    let mut seen = HashSet::new();

    for item in &parsed.scores {
        let criterion_id: CriterionId = item
            .criterion
            .parse()
            .map_err(|_| ParseError::UnknownCriterion(item.criterion.clone()))?;

        if !seen.insert(criterion_id) {
            return Err(ParseError::DuplicateCriterion(item.criterion.clone()));
        }

        let def = definitions
            .iter()
            .find(|d| d.id == criterion_id)
            .ok_or_else(|| ParseError::UnknownCriterion(item.criterion.clone()))?;

        if item.level < 1 || item.level > 5 {
            return Err(ParseError::LevelOutOfRange(item.level));
        }

        let weight = def.weight_for_level(item.level);

        scores.push(CriterionScore {
            criterion_id,
            level: item.level,
            weighted_score: item.level as f32 * weight,
            rationale: item.rationale.clone(),
            evidence: item.evidence.clone(),
            suggestions: item.suggestions.clone(),
        });
    }

    // Check that all expected criteria are present
    let missing: Vec<String> = definitions
        .iter()
        .filter(|d| !scores.iter().any(|s| s.criterion_id == d.id))
        .map(|d| d.id.to_string())
        .collect();

    if !missing.is_empty() {
        return Err(ParseError::MissingCriteria(missing.join(", ")));
    }

    // Sort to match definitions order for deterministic output
    scores.sort_by_key(|s| {
        definitions
            .iter()
            .position(|d| d.id == s.criterion_id)
            .unwrap_or(usize::MAX)
    });

    Ok(scores)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assessment::criteria::{atomicity, cohesion, CriterionId};

    #[test]
    fn parses_valid_batched_response() {
        let response = r#"{"scores": [
            {"criterion": "atomicity", "level": 4, "rationale": "Single logical change", "evidence": ["One purpose"], "suggestions": []}
        ]}"#;
        let defs = vec![atomicity::definition()];

        let scores = parse_assessment_response(response, &defs).unwrap();

        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].level, 4);
        assert_eq!(scores[0].criterion_id, CriterionId::Atomicity);
    }

    #[test]
    fn parses_multi_criterion_response() {
        let response = r#"{"scores": [
            {"criterion": "atomicity", "level": 4, "rationale": "Good", "evidence": ["a"], "suggestions": []},
            {"criterion": "logical_cohesion", "level": 3, "rationale": "Adequate", "evidence": ["b"], "suggestions": ["improve"]}
        ]}"#;
        let defs = vec![atomicity::definition(), cohesion::definition()];

        let scores = parse_assessment_response(response, &defs).unwrap();

        assert_eq!(scores.len(), 2);
        assert_eq!(scores[0].criterion_id, CriterionId::Atomicity);
        assert_eq!(scores[1].criterion_id, CriterionId::LogicalCohesion);
    }

    #[test]
    fn parses_response_with_markdown() {
        let response = r#"```json
{"scores": [{"criterion": "atomicity", "level": 3, "rationale": "Adequate", "evidence": ["context"], "suggestions": ["focus"]}]}
```"#;
        let defs = vec![atomicity::definition()];

        let scores = parse_assessment_response(response, &defs).unwrap();

        assert_eq!(scores[0].level, 3);
    }

    #[test]
    fn rejects_invalid_level() {
        let response = r#"{"scores": [{"criterion": "atomicity", "level": 7, "rationale": "Test", "evidence": [], "suggestions": []}]}"#;
        let defs = vec![atomicity::definition()];

        let result = parse_assessment_response(response, &defs);
        assert!(matches!(result, Err(ParseError::LevelOutOfRange(7))));
    }

    #[test]
    fn rejects_missing_criteria() {
        let response = r#"{"scores": [
            {"criterion": "atomicity", "level": 4, "rationale": "Good", "evidence": [], "suggestions": []}
        ]}"#;
        // Expect both atomicity and cohesion
        let defs = vec![atomicity::definition(), cohesion::definition()];

        let result = parse_assessment_response(response, &defs);
        assert!(matches!(result, Err(ParseError::MissingCriteria(_))));
        if let Err(ParseError::MissingCriteria(msg)) = result {
            assert!(msg.contains("logical_cohesion"));
        }
    }

    #[test]
    fn rejects_unknown_criterion() {
        let response = r#"{"scores": [{"criterion": "nonexistent", "level": 3, "rationale": "Test", "evidence": [], "suggestions": []}]}"#;
        let defs = vec![atomicity::definition()];

        let result = parse_assessment_response(response, &defs);
        assert!(matches!(result, Err(ParseError::UnknownCriterion(_))));
    }

    #[test]
    fn rejects_duplicate_criterion() {
        let response = r#"{"scores": [
            {"criterion": "atomicity", "level": 4, "rationale": "Good", "evidence": [], "suggestions": []},
            {"criterion": "atomicity", "level": 3, "rationale": "Also good", "evidence": [], "suggestions": []}
        ]}"#;
        let defs = vec![atomicity::definition()];

        let result = parse_assessment_response(response, &defs);
        assert!(matches!(result, Err(ParseError::DuplicateCriterion(_))));
    }

    #[test]
    fn sorts_scores_to_match_definitions_order() {
        // Response has cohesion before atomicity, but definitions have atomicity first
        let response = r#"{"scores": [
            {"criterion": "logical_cohesion", "level": 3, "rationale": "Ok", "evidence": [], "suggestions": []},
            {"criterion": "atomicity", "level": 4, "rationale": "Good", "evidence": [], "suggestions": []}
        ]}"#;
        let defs = vec![atomicity::definition(), cohesion::definition()];

        let scores = parse_assessment_response(response, &defs).unwrap();
        assert_eq!(scores[0].criterion_id, CriterionId::Atomicity);
        assert_eq!(scores[1].criterion_id, CriterionId::LogicalCohesion);
    }

    #[test]
    fn rejects_no_json() {
        let response = "This response has no JSON";
        let defs = vec![atomicity::definition()];

        let result = parse_assessment_response(response, &defs);
        assert!(matches!(result, Err(ParseError::NoJson)));
    }
}
