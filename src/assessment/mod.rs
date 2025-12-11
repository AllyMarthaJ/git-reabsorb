//! Commit range assessment module.
//!
//! This module provides tools to assess commits against a rubric of criteria,
//! with LLM-based assessment and before/after comparison support.

pub mod comparison;
pub mod criteria;
pub mod llm;
pub mod report;
pub mod types;

pub use comparison::{compare_assessments, load_assessment, save_assessment};
pub use criteria::{AssessmentError, Criterion, CriterionId, RangeContext};
pub use types::{
    AggregateScore, AssessmentComparison, AssessmentLevel, CommitAssessment, CriterionScore,
    RangeAssessment,
};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::git::GitOps;
use crate::llm::LlmClient;
use crate::models::SourceCommit;

use criteria::get_definition;
use llm::LlmCriterionAssessor;

/// Main assessment engine for evaluating commit quality.
pub struct AssessmentEngine {
    client: Arc<dyn LlmClient>,
    criterion_ids: Vec<CriterionId>,
    max_parallel: usize,
}

impl AssessmentEngine {
    /// Create an engine with specific criteria.
    pub fn new(client: Arc<dyn LlmClient>, criterion_ids: &[CriterionId]) -> Self {
        Self {
            client,
            criterion_ids: criterion_ids.to_vec(),
            max_parallel: 4, // Default parallelism for commits
        }
    }

    /// Create an engine with all default criteria.
    pub fn with_all_criteria(client: Arc<dyn LlmClient>) -> Self {
        Self::new(client, CriterionId::all())
    }

    /// Set maximum parallel commit assessments.
    pub fn with_parallelism(mut self, max_parallel: usize) -> Self {
        self.max_parallel = max_parallel;
        self
    }

    /// Assess a range of commits in parallel.
    pub fn assess_range<G: GitOps>(
        &self,
        git: &G,
        base_sha: &str,
        head_sha: &str,
        commits: &[SourceCommit],
    ) -> Result<RangeAssessment, AssessmentError> {
        let total = commits.len();

        // Collect all files changed in the range for context
        let files_in_range = self.collect_files_in_range(git, commits);

        // Pre-fetch all diffs (git operations are fast, do sequentially)
        eprintln!("Fetching diffs for {} commits...", total);
        let mut commit_data: Vec<(usize, SourceCommit, String)> = Vec::new();
        for (position, commit) in commits.iter().enumerate() {
            let diff_content = self.get_diff_content(git, &commit.sha)?;
            commit_data.push((position, commit.clone(), diff_content));
        }

        // Assess commits in parallel batches
        eprintln!(
            "Assessing {} commits ({} parallel)...",
            total, self.max_parallel
        );

        let results: Arc<Mutex<Vec<CommitAssessment>>> = Arc::new(Mutex::new(Vec::new()));
        let errors: Arc<Mutex<Vec<(usize, AssessmentError)>>> = Arc::new(Mutex::new(Vec::new()));

        let chunks: Vec<_> = commit_data.chunks(self.max_parallel).collect();

        for chunk in chunks {
            let handles: Vec<_> = chunk
                .iter()
                .map(|(position, commit, diff_content)| {
                    let client = Arc::clone(&self.client);
                    let criterion_ids = self.criterion_ids.clone();
                    let results = Arc::clone(&results);
                    let errors = Arc::clone(&errors);
                    let commits_clone = commits.to_vec();
                    let files_clone = files_in_range.clone();
                    let position = *position;
                    let commit = commit.clone();
                    let diff_content = diff_content.clone();

                    thread::spawn(move || {
                        eprintln!(
                            "  [{}/{}] {} {}",
                            position + 1,
                            total,
                            &commit.sha[..8.min(commit.sha.len())],
                            commit.message.short
                        );

                        match assess_single_commit(
                            &client,
                            &criterion_ids,
                            &commit,
                            &diff_content,
                            &commits_clone,
                            &files_clone,
                            position,
                            total,
                        ) {
                            Ok(assessment) => {
                                let mut results = results.lock().unwrap();
                                results.push(assessment);
                            }
                            Err(e) => {
                                let mut errors = errors.lock().unwrap();
                                errors.push((position, e));
                            }
                        }
                    })
                })
                .collect();

            // Wait for this batch to complete
            for handle in handles {
                let _ = handle.join();
            }
        }

        // Check for errors
        let errors = Arc::try_unwrap(errors).unwrap().into_inner().unwrap();
        if let Some((position, error)) = errors.into_iter().next() {
            eprintln!("Assessment failed at commit {}", position);
            return Err(error);
        }

        // Sort results by position (they may be out of order due to parallelism)
        let mut commit_assessments = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
        commit_assessments.sort_by_key(|ca| ca.position);

        let aggregate_scores = self.calculate_aggregates(&commit_assessments);
        let overall_score = if commit_assessments.is_empty() {
            0.0
        } else {
            commit_assessments
                .iter()
                .map(|ca| ca.overall_score)
                .sum::<f32>()
                / commit_assessments.len() as f32
        };

        Ok(RangeAssessment {
            base_sha: base_sha.to_string(),
            head_sha: head_sha.to_string(),
            assessed_at: chrono::Utc::now().to_rfc3339(),
            commit_assessments,
            aggregate_scores,
            overall_score,
            range_observations: Vec::new(),
        })
    }

    fn get_diff_content<G: GitOps>(&self, git: &G, sha: &str) -> Result<String, AssessmentError> {
        let hunks = git
            .read_hunks(sha, 0)
            .map_err(|e| AssessmentError::GitError(e.to_string()))?;

        Ok(hunks
            .iter()
            .map(|h| h.to_patch())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn collect_files_in_range<G: GitOps>(&self, git: &G, commits: &[SourceCommit]) -> Vec<String> {
        let mut files = Vec::new();
        for commit in commits {
            if let Ok(changed) = git.get_files_changed_in_commit(&commit.sha) {
                for file in changed {
                    if !files.contains(&file) {
                        files.push(file);
                    }
                }
            }
        }
        files
    }

    fn calculate_aggregates(
        &self,
        assessments: &[CommitAssessment],
    ) -> HashMap<String, AggregateScore> {
        let mut aggregates = HashMap::new();

        for criterion_id in &self.criterion_ids {
            let def = get_definition(*criterion_id);
            let scores: Vec<f32> = assessments
                .iter()
                .filter_map(|ca| {
                    ca.criterion_scores
                        .iter()
                        .find(|s| s.criterion_id == def.id.to_string())
                        .map(|s| s.level as f32)
                })
                .collect();

            if scores.is_empty() {
                continue;
            }

            let mean = scores.iter().sum::<f32>() / scores.len() as f32;
            let min = scores.iter().cloned().fold(f32::INFINITY, f32::min);
            let max = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let variance =
                scores.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / scores.len() as f32;

            aggregates.insert(
                def.id.to_string(),
                AggregateScore {
                    criterion_id: def.id.to_string(),
                    criterion_name: def.name.clone(),
                    mean_score: mean,
                    min_score: min,
                    max_score: max,
                    std_deviation: variance.sqrt(),
                },
            );
        }

        aggregates
    }
}

/// Assess a single commit against all criteria (called from thread).
#[allow(clippy::too_many_arguments)]
fn assess_single_commit(
    client: &Arc<dyn LlmClient>,
    criterion_ids: &[CriterionId],
    commit: &SourceCommit,
    diff_content: &str,
    all_commits: &[SourceCommit],
    files_in_range: &[String],
    position: usize,
    total: usize,
) -> Result<CommitAssessment, AssessmentError> {
    let range_context =
        RangeContext::new(all_commits.to_vec(), position).with_files(files_in_range.to_vec());

    let mut criterion_scores = Vec::new();
    let mut max_possible: f32 = 0.0;

    for criterion_id in criterion_ids {
        let assessor = LlmCriterionAssessor::for_criterion(client.clone(), *criterion_id);
        let score = assessor.assess(commit, diff_content, &range_context)?;
        max_possible += assessor.definition().max_weighted_score();
        criterion_scores.push(score);
    }

    let total_weighted: f32 = criterion_scores.iter().map(|s| s.weighted_score).sum();
    let overall_score = if max_possible > 0.0 {
        total_weighted / max_possible
    } else {
        0.0
    };

    Ok(CommitAssessment {
        commit_sha: commit.sha.clone(),
        commit_message: commit.message.short.clone(),
        criterion_scores,
        overall_score,
        position,
        total_commits: total,
    })
}

/// Get definitions for specific criterion IDs.
pub fn get_definitions(ids: &[CriterionId]) -> Vec<criteria::CriterionDefinition> {
    ids.iter().map(|id| get_definition(*id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criterion_id_all() {
        let all = CriterionId::all();
        assert_eq!(all.len(), 5);
    }
}
