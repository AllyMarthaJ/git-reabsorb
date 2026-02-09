//! LLM-based commit assessment.

pub mod parser;
pub mod prompt;

use std::sync::Arc;

use log::debug;

use crate::assessment::criteria::{
    get_definition, AssessmentError, CriterionDefinition, CriterionId, RangeContext,
};
use crate::assessment::types::{CommitAssessment, CriterionScore};
use crate::llm::LlmClient;
use crate::models::SourceCommit;

/// LLM-based assessor that evaluates all criteria in a single call.
pub struct LlmAssessor {
    client: Arc<dyn LlmClient>,
    definitions: Vec<CriterionDefinition>,
    max_retries: usize,
    max_context_commits: usize,
}

impl LlmAssessor {
    /// Create an assessor for specific criterion IDs.
    pub fn new(
        client: Arc<dyn LlmClient>,
        criterion_ids: &[CriterionId],
        max_context_commits: usize,
    ) -> Self {
        let definitions = criterion_ids.iter().map(|id| get_definition(*id)).collect();
        Self {
            client,
            definitions,
            max_retries: 3,
            max_context_commits,
        }
    }

    /// Set the maximum number of retries on failure.
    pub fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Maximum possible weighted score across all criteria.
    fn max_possible_score(&self) -> f32 {
        self.definitions
            .iter()
            .map(|d| d.max_weighted_score())
            .sum()
    }

    /// Assess a single commit against all criteria in one LLM call.
    pub fn assess_commit(
        &self,
        commit: &SourceCommit,
        diff_content: &str,
        range_context: &RangeContext,
        position: usize,
        total: usize,
    ) -> Result<CommitAssessment, AssessmentError> {
        let prompt_text = prompt::build_assessment_prompt(
            &self.definitions,
            commit,
            diff_content,
            range_context,
            self.max_context_commits,
        );

        let mut last_error = None;

        for attempt in 1..=self.max_retries {
            match self.client.complete(&prompt_text) {
                Ok(response) => {
                    match parser::parse_assessment_response(&response, &self.definitions) {
                        Ok(criterion_scores) => {
                            return Ok(self.build_assessment(
                                commit,
                                criterion_scores,
                                position,
                                total,
                            ));
                        }
                        Err(e) if attempt < self.max_retries => {
                            debug!(
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
                    debug!(
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

    fn build_assessment(
        &self,
        commit: &SourceCommit,
        criterion_scores: Vec<CriterionScore>,
        position: usize,
        total: usize,
    ) -> CommitAssessment {
        let total_weighted: f32 = criterion_scores.iter().map(|s| s.weighted_score).sum();
        let max_possible = self.max_possible_score();
        let overall_score = if max_possible > 0.0 {
            total_weighted / max_possible
        } else {
            0.0
        };

        CommitAssessment {
            commit_sha: commit.sha.clone(),
            commit_message: commit.message.short.clone(),
            criterion_scores,
            overall_score,
            position,
            total_commits: total,
        }
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
            r#"{"scores": [{"criterion": "atomicity", "level": 4, "rationale": "Good atomicity", "evidence": ["Single change"], "suggestions": []}]}"#,
        ));

        let assessor = LlmAssessor::new(client, &[CriterionId::Atomicity], 10);
        let commit = SourceCommit::new("abc123", "Add feature", "Add feature\n\nDetails");
        let context = RangeContext::new(vec![commit.clone()], 0);

        let assessment = assessor
            .assess_commit(&commit, "+code", &context, 0, 1)
            .unwrap();

        assert_eq!(assessment.criterion_scores.len(), 1);
        assert_eq!(assessment.criterion_scores[0].level, 4);
        assert_eq!(
            assessment.criterion_scores[0].criterion_id,
            CriterionId::Atomicity
        );
        assert_eq!(assessment.position, 0);
        assert_eq!(assessment.total_commits, 1);
    }

    #[test]
    fn assesses_multiple_criteria() {
        let client = Arc::new(MockLlmClient::new(
            r#"{"scores": [
                {"criterion": "atomicity", "level": 4, "rationale": "Good", "evidence": ["a"], "suggestions": []},
                {"criterion": "message_quality", "level": 3, "rationale": "Adequate", "evidence": ["b"], "suggestions": ["improve"]}
            ]}"#,
        ));

        let assessor = LlmAssessor::new(
            client,
            &[CriterionId::Atomicity, CriterionId::MessageQuality],
            10,
        );
        let commit = SourceCommit::new("abc123", "Add feature", "Add feature\n\nDetails");
        let context = RangeContext::new(vec![commit.clone()], 0);

        let assessment = assessor
            .assess_commit(&commit, "+code", &context, 0, 1)
            .unwrap();

        assert_eq!(assessment.criterion_scores.len(), 2);
        assert!(assessment.overall_score > 0.0);
    }
}
