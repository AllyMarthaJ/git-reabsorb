//! Response parsing for LLM-based commit assessment.

use serde::Deserialize;

use crate::assessment::criteria::CriterionDefinition;
use crate::assessment::types::CriterionScore;
use crate::utils::extract_json_str;

/// Raw LLM response structure.
#[derive(Debug, Deserialize)]
struct LlmAssessmentResponse {
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
}

/// Parse the LLM response into a CriterionScore.
pub fn parse_criterion_response(
    response: &str,
    definition: &CriterionDefinition,
) -> Result<CriterionScore, ParseError> {
    let json_str = extract_json_str(response).ok_or(ParseError::NoJson)?;

    let parsed: LlmAssessmentResponse =
        serde_json::from_str(json_str).map_err(|e| ParseError::InvalidJson(e.to_string()))?;

    // Validate level
    if parsed.level < 1 || parsed.level > 5 {
        return Err(ParseError::LevelOutOfRange(parsed.level));
    }

    let weight = definition.weight_for_level(parsed.level);

    Ok(CriterionScore {
        criterion_id: definition.id,
        level: parsed.level,
        weighted_score: parsed.level as f32 * weight,
        rationale: parsed.rationale,
        evidence: parsed.evidence,
        suggestions: parsed.suggestions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assessment::criteria::{atomicity, CriterionId};

    #[test]
    fn parses_valid_response() {
        let response = r#"{"level": 4, "rationale": "Single logical change", "evidence": ["One purpose"], "suggestions": []}"#;
        let def = atomicity::definition();

        let score = parse_criterion_response(response, &def).unwrap();

        assert_eq!(score.level, 4);
        assert_eq!(score.weighted_score, 4.0);
        assert_eq!(score.criterion_id, CriterionId::Atomicity);
    }

    #[test]
    fn parses_response_with_markdown() {
        let response = r#"```json
{"level": 3, "rationale": "Adequate", "evidence": ["Some context"], "suggestions": ["Be more specific"]}
```"#;
        let def = atomicity::definition();

        let score = parse_criterion_response(response, &def).unwrap();

        assert_eq!(score.level, 3);
        assert!(!score.suggestions.is_empty());
    }

    #[test]
    fn rejects_invalid_level() {
        let response = r#"{"level": 7, "rationale": "Test", "evidence": [], "suggestions": []}"#;
        let def = atomicity::definition();

        let result = parse_criterion_response(response, &def);

        assert!(matches!(result, Err(ParseError::LevelOutOfRange(7))));
    }

    #[test]
    fn rejects_no_json() {
        let response = "This response has no JSON";
        let def = atomicity::definition();

        let result = parse_criterion_response(response, &def);

        assert!(matches!(result, Err(ParseError::NoJson)));
    }
}
