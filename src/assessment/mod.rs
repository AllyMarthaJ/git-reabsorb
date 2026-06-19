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
pub use criteria::{AssessmentError, CriterionId, DiffStats, RangeContext};
pub use types::{
    AggregateScore, AssessmentComparison, AssessmentLevel, CommitAssessment, CriterionScore,
    RangeAssessment,
};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use log::{debug, error, info};

use crate::extract::ExtractedCommit;
use crate::git::GitOps;
use crate::llm::LlmClient;
use crate::models::SourceCommit;

use criteria::{compute_scope_score, get_definition};
use llm::LlmAssessor;

/// Main assessment engine for evaluating commit quality.
pub struct AssessmentEngine {
    client: Arc<dyn LlmClient>,
    criterion_ids: Vec<CriterionId>,
    max_parallel: usize,
    max_context_commits: usize,
}

impl AssessmentEngine {
    /// Create an engine with specific criteria.
    pub fn new(client: Arc<dyn LlmClient>, criterion_ids: &[CriterionId]) -> Self {
        Self {
            client,
            criterion_ids: criterion_ids.to_vec(),
            max_parallel: 4,
            max_context_commits: 10,
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

    /// Set maximum context commits shown in prompts.
    pub fn with_max_context_commits(mut self, max_context_commits: usize) -> Self {
        self.max_context_commits = max_context_commits;
        self
    }

    /// Criterion IDs that should be assessed by the LLM (excludes deterministic ones).
    fn llm_criterion_ids(&self) -> Vec<CriterionId> {
        self.criterion_ids
            .iter()
            .filter(|id| **id != CriterionId::ScopeAppropriateness)
            .copied()
            .collect()
    }

    /// Whether scope should be computed deterministically.
    fn includes_scope(&self) -> bool {
        self.criterion_ids
            .contains(&CriterionId::ScopeAppropriateness)
    }

    /// Assess a range of commits by fetching data from git first.
    pub fn assess_range<G: GitOps>(
        &self,
        git: &G,
        base_sha: &str,
        head_sha: &str,
        commits: &[SourceCommit],
    ) -> Result<RangeAssessment, AssessmentError> {
        // Pre-fetch all diffs and build ExtractedCommit records
        info!("Fetching diffs for {} commits...", commits.len());
        let mut extracted: Vec<ExtractedCommit> = Vec::new();
        for commit in commits {
            let (diff_content, diff_stats) = self.get_diff_content_and_stats(git, &commit.sha)?;
            let files_changed = git
                .get_files_changed_in_commit(&commit.sha)
                .unwrap_or_default();

            let body = if commit.message.long.len() > commit.message.short.len() {
                commit.message.long[commit.message.short.len()..]
                    .trim()
                    .to_string()
            } else {
                String::new()
            };

            extracted.push(ExtractedCommit {
                hash: commit.sha.clone(),
                subject: commit.message.short.clone(),
                body,
                author_name: String::new(),
                author_date: String::new(),
                diff: diff_content,
                diff_stat: diff_stats,
                files_changed,
                diff_truncated: false,
            });
        }

        let commit_assessments = self.assess_commits(&extracted)?;

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

    /// Assess a set of extracted commits in parallel.
    ///
    /// This is the core assessment method — both git-backed (`assess_range`) and
    /// file-backed (`--from-file`) modes funnel through here.
    pub fn assess_commits(
        &self,
        commits: &[ExtractedCommit],
    ) -> Result<Vec<CommitAssessment>, AssessmentError> {
        let total = commits.len();

        // Build SourceCommit list for range context
        let source_commits: Vec<SourceCommit> = commits
            .iter()
            .map(|c| {
                let long = if c.body.is_empty() {
                    c.subject.clone()
                } else {
                    format!("{}\n\n{}", c.subject, c.body)
                };
                SourceCommit::new(&c.hash, &c.subject, long)
            })
            .collect();

        // Collect all files for range context
        let files_in_range: Vec<String> = {
            let mut files = Vec::new();
            for c in commits {
                for f in &c.files_changed {
                    if !files.contains(f) {
                        files.push(f.clone());
                    }
                }
            }
            files
        };

        // Create assessor
        let llm_ids = self.llm_criterion_ids();
        let assessor = Arc::new(LlmAssessor::new(
            Arc::clone(&self.client),
            &llm_ids,
            self.max_context_commits,
        ));

        let includes_scope = self.includes_scope();

        info!(
            "Assessing {} commits ({} parallel)...",
            total, self.max_parallel
        );

        let results: Arc<Mutex<Vec<CommitAssessment>>> = Arc::new(Mutex::new(Vec::new()));
        let errors: Arc<Mutex<Vec<(usize, AssessmentError)>>> = Arc::new(Mutex::new(Vec::new()));

        let indexed: Vec<_> = commits.iter().enumerate().collect();
        let chunks: Vec<_> = indexed.chunks(self.max_parallel).collect();

        for chunk in chunks {
            let handles: Vec<_> = chunk
                .iter()
                .map(|(position, commit)| {
                    let assessor = Arc::clone(&assessor);
                    let results = Arc::clone(&results);
                    let errors = Arc::clone(&errors);
                    let source_commits_clone = source_commits.clone();
                    let files_clone = files_in_range.clone();
                    let position = *position;
                    let source_commit = source_commits[position].clone();
                    let diff_content = commit.diff.clone();
                    let diff_stats = commit.diff_stat.clone();

                    thread::spawn(move || {
                        debug!(
                            "[{}/{}] {} {}",
                            position + 1,
                            total,
                            &source_commit.sha[..8.min(source_commit.sha.len())],
                            source_commit.message.short
                        );

                        let range_context = RangeContext::new(source_commits_clone, position)
                            .with_files(files_clone);

                        match assessor.assess_commit(
                            &source_commit,
                            &diff_content,
                            &diff_stats,
                            &range_context,
                            position,
                            total,
                        ) {
                            Ok(mut assessment) => {
                                if includes_scope {
                                    let scope_score = compute_scope_score(&diff_stats);
                                    assessment.criterion_scores.push(scope_score);
                                    let total_weighted: f32 = assessment
                                        .criterion_scores
                                        .iter()
                                        .map(|s| s.weighted_score)
                                        .sum();
                                    let max_possible: f32 = assessment
                                        .criterion_scores
                                        .iter()
                                        .map(|s| {
                                            get_definition(s.criterion_id).max_weighted_score()
                                        })
                                        .sum();
                                    assessment.overall_score = if max_possible > 0.0 {
                                        total_weighted / max_possible
                                    } else {
                                        0.0
                                    };
                                }
                                results.lock().unwrap().push(assessment);
                            }
                            Err(e) => {
                                errors.lock().unwrap().push((position, e));
                            }
                        }
                    })
                })
                .collect();

            for handle in handles {
                let _ = handle.join();
            }
        }

        let errors = Arc::try_unwrap(errors).unwrap().into_inner().unwrap();
        if let Some((position, error)) = errors.into_iter().next() {
            error!("Assessment failed at commit {}", position);
            return Err(error);
        }

        let mut commit_assessments = Arc::try_unwrap(results).unwrap().into_inner().unwrap();
        commit_assessments.sort_by_key(|ca| ca.position);

        Ok(commit_assessments)
    }

    fn get_diff_content_and_stats<G: GitOps>(
        &self,
        git: &G,
        sha: &str,
    ) -> Result<(String, DiffStats), AssessmentError> {
        let hunks = git
            .read_hunks(sha, 0)
            .map_err(|e| AssessmentError::GitError(e.to_string()))?;

        let diff_content = hunks
            .iter()
            .map(|h| h.to_patch())
            .collect::<Vec<_>>()
            .join("\n");

        // Compute stats from hunks
        let mut lines_added = 0usize;
        let mut lines_removed = 0usize;
        let mut files: Vec<String> = Vec::new();

        for hunk in &hunks {
            let patch = hunk.to_patch();
            for line in patch.lines() {
                if line.starts_with('+') && !line.starts_with("+++") {
                    lines_added += 1;
                } else if line.starts_with('-') && !line.starts_with("---") {
                    lines_removed += 1;
                }
            }
            // Count unique files from diff headers
            for line in patch.lines() {
                if let Some(path) = line.strip_prefix("+++ b/") {
                    if !files.contains(&path.to_string()) {
                        files.push(path.to_string());
                    }
                }
            }
        }

        let diff_stats = DiffStats {
            lines_added,
            lines_removed,
            files_changed: files.len(),
        };

        Ok((diff_content, diff_stats))
    }

    pub fn calculate_aggregates(
        &self,
        assessments: &[CommitAssessment],
    ) -> HashMap<CriterionId, AggregateScore> {
        let mut aggregates = HashMap::new();

        for criterion_id in &self.criterion_ids {
            let def = get_definition(*criterion_id);
            let scores: Vec<f32> = assessments
                .iter()
                .filter_map(|ca| {
                    ca.criterion_scores
                        .iter()
                        .find(|s| s.criterion_id == def.id)
                        .map(|s| s.weighted_score)
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
                def.id,
                AggregateScore {
                    criterion_id: def.id,
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
        assert_eq!(all.len(), 4);
    }
}
