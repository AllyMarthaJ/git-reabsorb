//! LLM-based criterion assessment.

pub mod parser;
pub mod prompt;

use std::sync::Arc;

use crate::assessment::criteria::{
    get_definition, AssessmentError, Criterion, CriterionDefinition, CriterionId, RangeContext,
};
use crate::assessment::types::CriterionScore;
use crate::llm::LlmClient;
use crate::models::SourceCommit;

/// LLM-based criterion assessor.
///
/// Implements the `Criterion` trait by delegating assessment to an LLM.
pub struct LlmCriterionAssessor {
    client: Arc<dyn LlmClient>,
    definition: CriterionDefinition,
    max_retries: usize,
}

impl LlmCriterionAssessor {
    /// Create a new LLM criterion assessor with a custom definition.
    pub fn new(client: Arc<dyn LlmClient>, definition: CriterionDefinition) -> Self {
        Self {
            client,
            definition,
            max_retries: 3,
        }
    }

    /// Create an LLM assessor for a specific criterion ID.
    pub fn for_criterion(client: Arc<dyn LlmClient>, id: CriterionId) -> Self {
        let definition = get_definition(id);
        Self::new(client, definition)
    }

    /// Set the maximum number of retries on failure.
    pub fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }
}

impl Criterion for LlmCriterionAssessor {
    fn definition(&self) -> &CriterionDefinition {
        &self.definition
    }

    fn assess(
        &self,
        commit: &SourceCommit,
        diff_content: &str,
        range_context: &RangeContext,
    ) -> Result<CriterionScore, AssessmentError> {
        let prompt_text =
            prompt::build_assessment_prompt(&self.definition, commit, diff_content, range_context);

        let mut last_error = None;

        for attempt in 1..=self.max_retries {
            match self.client.complete(&prompt_text) {
                Ok(response) => {
                    match parser::parse_criterion_response(&response, &self.definition) {
                        Ok(score) => return Ok(score),
                        Err(e) if attempt < self.max_retries => {
                            eprintln!(
                                "Parse error (attempt {}/{}): {}",
                                attempt, self.max_retries, e
                            );
                            last_error = Some(AssessmentError::InvalidResponse(e.to_string()));
                            continue;
                        }
                        Err(e) => {
                            return Err(AssessmentError::InvalidResponse(e.to_string()));
                        }
                    }
                }
                Err(e) if attempt < self.max_retries => {
                    eprintln!(
                        "LLM error (attempt {}/{}): {}",
                        attempt, self.max_retries, e
                    );
                    last_error = Some(AssessmentError::LlmFailed(e.to_string()));
                    continue;
                }
                Err(e) => {
                    return Err(AssessmentError::LlmFailed(e.to_string()));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| AssessmentError::LlmFailed("Max retries exceeded".into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmError;

    struct MockLlmClient {
        response: String,
    }

    impl MockLlmClient {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
            }
        }
    }

    impl LlmClient for MockLlmClient {
        fn complete(&self, _prompt: &str) -> Result<String, LlmError> {
            Ok(self.response.clone())
        }
    }

    #[test]
    fn assesses_with_mock_client() {
        let client = Arc::new(MockLlmClient::new(
            r#"{"level": 4, "rationale": "Good atomicity", "evidence": ["Single change"], "suggestions": []}"#,
        ));

        let assessor = LlmCriterionAssessor::for_criterion(client, CriterionId::Atomicity);
        let commit = SourceCommit::new("abc123", "Add feature", "Add feature\n\nDetails");
        let context = RangeContext::new(vec![commit.clone()], 0);

        let score = assessor.assess(&commit, "+code", &context).unwrap();

        assert_eq!(score.level, 4);
        assert_eq!(score.criterion_id, "atomicity");
    }
}
