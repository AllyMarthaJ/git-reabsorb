use std::collections::HashMap;
use std::sync::Arc;

use log::{debug, warn};

use crate::assessment::criteria::DiffStats;
use crate::assessment::{AssessmentEngine, CriterionId};
use crate::extract::ExtractedCommit;
use crate::features::Feature;
use crate::git::{GitError, GitOps};
use crate::llm::LlmClient;
use crate::models::{DiffLine, FileChange, Hunk, PlannedChange, PlannedCommit, SourceCommit, Strategy};
use crate::patch::{parse, ParseError, Patch};
use crate::reorganize::ReorganizeError;
use crate::validation::{validate_plan, ValidationIssue};

use super::StrategyFactory;

/// MessageQuality level threshold below which a planned commit is considered failed.
/// Level 4 ("names system + decision; body explains reasoning concisely") is the bar.
const ASSESSMENT_FAIL_LEVEL: u8 = 4;

/// Creates commit plans from source commits and hunks.
pub struct Planner<'a, G: GitOps> {
    git: &'a G,
    strategies: StrategyFactory,
    max_fix_attempts: usize,
    llm_client: Option<Arc<dyn LlmClient>>,
}

impl<'a, G: GitOps> Planner<'a, G> {
    pub fn new(git: &'a G, strategies: StrategyFactory) -> Self {
        Self {
            git,
            strategies,
            max_fix_attempts: 3,
            llm_client: None,
        }
    }

    pub fn with_max_fix_attempts(mut self, max_fix_attempts: usize) -> Self {
        self.max_fix_attempts = max_fix_attempts;
        self
    }

    /// Attach an LLM client used by `Feature::AssessPlannedCommits` to score
    /// each planned commit's MessageQuality during validation.
    pub fn with_llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.llm_client = Some(client);
        self
    }

    pub fn read_source_commits(
        &self,
        base: &str,
        head: &str,
    ) -> Result<Vec<SourceCommit>, GitError> {
        self.git.read_commits(base, head)
    }

    pub fn build_file_to_commits_map(
        &self,
        source_commits: &[SourceCommit],
    ) -> Result<HashMap<String, Vec<String>>, GitError> {
        let mut file_to_commits: HashMap<String, Vec<String>> = HashMap::new();

        for commit in source_commits {
            for file in self.git.get_files_changed_in_commit(&commit.sha)? {
                file_to_commits
                    .entry(file)
                    .or_default()
                    .push(commit.sha.clone());
            }
        }

        Ok(file_to_commits)
    }

    pub fn parse_diff_full_with_commit_mapping(
        &self,
        diff_output: &str,
        file_to_commits: &HashMap<String, Vec<String>>,
    ) -> Result<(Vec<Hunk>, Vec<FileChange>), ParseError> {
        let Patch {
            mut hunks,
            mut file_changes,
        } = parse(diff_output, &[], 0)?;

        for hunk in &mut hunks {
            if let Some(commits) =
                file_to_commits.get(&hunk.file_path.to_string_lossy().to_string())
            {
                hunk.likely_source_commits.clone_from(commits);
            }
        }

        for file_change in &mut file_changes {
            if let Some(commits) =
                file_to_commits.get(&file_change.file_path.to_string_lossy().to_string())
            {
                file_change.likely_source_commits.clone_from(commits);
            }
        }

        Ok((hunks, file_changes))
    }

    pub fn draft_plan(
        &self,
        strategy: Strategy,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
        file_to_commits: &HashMap<String, Vec<String>>,
        file_changes: &[FileChange],
    ) -> Result<PlanDraft, ReorganizeError> {
        let reorganizer = self.strategies.create(strategy);
        let mut planned_commits = reorganizer.plan(source_commits, hunks)?;
        let removed_empty = retain_non_empty(&mut planned_commits);
        if removed_empty > 0 {
            debug!("Dropped {} empty commits from plan", removed_empty);
        }

        // Validate and fix loop with max retries
        for attempt in 0..self.max_fix_attempts {
            let mut validation = validate_plan(&planned_commits, hunks);
            self.augment_with_assessment(&mut validation, &planned_commits, hunks);

            if validation.is_valid() {
                break;
            }

            if attempt == self.max_fix_attempts - 1 {
                warn!(
                    "Plan still invalid after {} fix attempts: {:?}",
                    self.max_fix_attempts, validation.issues
                );
                break;
            }

            debug!(
                "Plan validation failed (attempt {}), attempting fix: {:?}",
                attempt + 1,
                validation.issues
            );

            planned_commits =
                reorganizer.fix_plan(planned_commits, &validation, source_commits, hunks)?;
            let removed = retain_non_empty(&mut planned_commits);
            if removed > 0 {
                debug!("Dropped {} empty commits after fix", removed);
            }
        }

        Ok(PlanDraft {
            strategy,
            planned_commits,
            hunks: hunks.to_vec(),
            file_to_commits: file_to_commits.clone(),
            file_changes: file_changes.to_vec(),
        })
    }

    /// If `Feature::AssessPlannedCommits` is on and a client is wired, score each
    /// planned commit's MessageQuality and append `FailedAssessment` issues for
    /// any that fall below `ASSESSMENT_FAIL_LEVEL`. The existing `fix_plan` branch
    /// in the LLM reorganizer consumes these via `build_fix_message_prompt`.
    fn augment_with_assessment(
        &self,
        validation: &mut crate::validation::ValidationResult,
        planned_commits: &[PlannedCommit],
        hunks: &[Hunk],
    ) {
        if !Feature::AssessPlannedCommits.is_enabled() {
            return;
        }
        let Some(client) = self.llm_client.clone() else {
            debug!("AssessPlannedCommits enabled but no LLM client wired; skipping");
            return;
        };
        if planned_commits.is_empty() {
            return;
        }

        let extracted: Vec<ExtractedCommit> = planned_commits
            .iter()
            .map(|c| build_extracted_for_planned(c, hunks))
            .collect();

        let engine = AssessmentEngine::new(client, &[CriterionId::MessageQuality]);
        match engine.assess_commits(&extracted) {
            Ok(assessments) => {
                for (commit, assessment) in planned_commits.iter().zip(assessments.iter()) {
                    let level = assessment
                        .criterion_scores
                        .iter()
                        .find(|s| s.criterion_id == CriterionId::MessageQuality)
                        .map(|s| s.level)
                        .unwrap_or(0);
                    if level < ASSESSMENT_FAIL_LEVEL {
                        debug!(
                            "  {} scored MessageQuality {}/5 — flagging for fix",
                            commit.id, level
                        );
                        validation.issues.push(ValidationIssue::FailedAssessment {
                            commit_id: commit.id,
                            assessment: assessment.clone(),
                        });
                    }
                }
            }
            Err(e) => {
                warn!("MessageQuality assessment failed; skipping: {}", e);
            }
        }
    }
}

/// Build an `ExtractedCommit` shape from a `PlannedCommit` so we can reuse the
/// assessment engine. SHA is synthetic (`planned-{id}`) and unused beyond logging.
fn build_extracted_for_planned(commit: &PlannedCommit, hunks: &[Hunk]) -> ExtractedCommit {
    let resolved: Vec<&Hunk> = commit
        .changes
        .iter()
        .filter_map(|c| match c {
            PlannedChange::ExistingHunk(id) => hunks.iter().find(|h| h.id == *id),
            PlannedChange::NewHunk(h) => Some(h),
        })
        .collect();

    let diff = resolved
        .iter()
        .map(|h| h.to_patch())
        .collect::<Vec<_>>()
        .join("\n");

    let mut lines_added = 0usize;
    let mut lines_removed = 0usize;
    for hunk in &resolved {
        for line in &hunk.lines {
            match line {
                DiffLine::Added(_) => lines_added += 1,
                DiffLine::Removed(_) => lines_removed += 1,
                DiffLine::Context(_) => {}
            }
        }
    }

    let mut files_changed: Vec<String> = Vec::new();
    for hunk in &resolved {
        let path = hunk.file_path.to_string_lossy().to_string();
        if !files_changed.contains(&path) {
            files_changed.push(path);
        }
    }

    let short = commit.description.short.clone();
    let body = if commit.description.long.starts_with(&short) {
        commit.description.long[short.len()..].trim().to_string()
    } else {
        commit.description.long.trim().to_string()
    };

    ExtractedCommit {
        hash: format!("planned-{}", commit.id.0),
        subject: short,
        body,
        author_name: String::new(),
        author_date: String::new(),
        diff,
        diff_stat: DiffStats {
            files_changed: files_changed.len(),
            lines_added,
            lines_removed,
        },
        files_changed,
        diff_truncated: false,
    }
}

pub struct PlanDraft {
    pub strategy: Strategy,
    pub planned_commits: Vec<PlannedCommit>,
    pub hunks: Vec<Hunk>,
    pub file_to_commits: HashMap<String, Vec<String>>,
    pub file_changes: Vec<FileChange>,
}

fn retain_non_empty(planned_commits: &mut Vec<PlannedCommit>) -> usize {
    let before = planned_commits.len();
    planned_commits.retain(|c| !c.changes.is_empty());
    before - planned_commits.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CommitDescription, HunkId, PlannedCommit, PlannedCommitId};

    #[test]
    fn drops_empty_commits() {
        let mut planned = vec![
            PlannedCommit::from_hunk_ids(
                PlannedCommitId(0),
                CommitDescription::short_only("keep"),
                vec![HunkId(1)],
            ),
            PlannedCommit::new(
                PlannedCommitId(1),
                CommitDescription::short_only("drop"),
                vec![],
            ),
        ];

        let removed = retain_non_empty(&mut planned);

        assert_eq!(removed, 1);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].description.short, "keep");
    }
}
