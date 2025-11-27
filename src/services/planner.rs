use std::collections::HashMap;

use crate::cli::StrategyArg;
use crate::diff_parser::{parse_diff, DiffParseError};
use crate::git::{GitError, GitOps};
use crate::models::{Hunk, PlannedCommit, SourceCommit};
use crate::reorganize::ReorganizeError;
use crate::services::strategy::StrategyFactory;

/// Creates commit plans from source commits and hunks.
pub struct Planner<'a, G: GitOps> {
    git: &'a G,
    strategies: StrategyFactory,
}

impl<'a, G: GitOps> Planner<'a, G> {
    pub fn new(git: &'a G, strategies: StrategyFactory) -> Self {
        Self { git, strategies }
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
    ) -> Result<(HashMap<String, Vec<String>>, HashMap<String, Vec<String>>), GitError> {
        let mut file_to_commits: HashMap<String, Vec<String>> = HashMap::new();
        let mut new_files_to_commits: HashMap<String, Vec<String>> = HashMap::new();

        for commit in source_commits {
            for file in self.git.get_files_changed_in_commit(&commit.sha)? {
                file_to_commits
                    .entry(file)
                    .or_default()
                    .push(commit.sha.clone());
            }
            for file in self.git.get_new_files_in_commit(&commit.sha)? {
                new_files_to_commits
                    .entry(file)
                    .or_default()
                    .push(commit.sha.clone());
            }
        }

        Ok((file_to_commits, new_files_to_commits))
    }

    pub fn read_hunks_from_source_commits(
        &self,
        source_commits: &[SourceCommit],
    ) -> Result<Vec<Hunk>, GitError> {
        let mut all_hunks = Vec::new();
        let mut hunk_id = 0;
        for commit in source_commits {
            let hunks = self.git.read_hunks(&commit.sha, hunk_id)?;
            hunk_id += hunks.len();
            all_hunks.extend(hunks);
        }
        Ok(all_hunks)
    }

    pub fn parse_diff_with_commit_mapping(
        &self,
        diff_output: &str,
        file_to_commits: &HashMap<String, Vec<String>>,
    ) -> Result<Vec<Hunk>, DiffParseError> {
        let mut hunks = parse_diff(diff_output, &[], 0)?;
        for hunk in &mut hunks {
            if let Some(commits) =
                file_to_commits.get(&hunk.file_path.to_string_lossy().to_string())
            {
                hunk.likely_source_commits.clone_from(commits);
            }
        }
        Ok(hunks)
    }

    pub fn draft_plan(
        &self,
        strategy: StrategyArg,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
        file_to_commits: &HashMap<String, Vec<String>>,
        new_files_to_commits: &HashMap<String, Vec<String>>,
    ) -> Result<PlanDraft, ReorganizeError> {
        let reorganizer = self.strategies.create(strategy);
        let strategy_name = reorganizer.name().to_string();
        let mut planned_commits = reorganizer.reorganize(source_commits, hunks)?;
        let removed_empty = retain_non_empty(&mut planned_commits);
        if removed_empty > 0 {
            eprintln!("Note: dropped {} empty commits from plan", removed_empty);
        }
        let new_hunks = extract_new_hunks(&planned_commits);

        Ok(PlanDraft {
            strategy_name,
            planned_commits,
            hunks: hunks.to_vec(),
            new_hunks,
            file_to_commits: file_to_commits.clone(),
            new_files_to_commits: new_files_to_commits.clone(),
        })
    }
}

/// Materialized plan details prior to persistence or execution.
pub struct PlanDraft {
    pub strategy_name: String,
    pub planned_commits: Vec<PlannedCommit>,
    pub hunks: Vec<Hunk>,
    pub new_hunks: Vec<Hunk>,
    pub file_to_commits: HashMap<String, Vec<String>>,
    pub new_files_to_commits: HashMap<String, Vec<String>>,
}

fn extract_new_hunks(planned_commits: &[PlannedCommit]) -> Vec<Hunk> {
    planned_commits
        .iter()
        .flat_map(|c| &c.changes)
        .filter_map(|change| {
            if let crate::models::PlannedChange::NewHunk(h) = change {
                Some(h.clone())
            } else {
                None
            }
        })
        .collect()
}

fn retain_non_empty(planned_commits: &mut Vec<PlannedCommit>) -> usize {
    let before = planned_commits.len();
    planned_commits.retain(|c| !c.changes.is_empty());
    before - planned_commits.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CommitDescription, HunkId, PlannedCommit};

    #[test]
    fn drops_empty_commits() {
        let mut planned = vec![
            PlannedCommit::from_hunk_ids(CommitDescription::short_only("keep"), vec![HunkId(1)]),
            PlannedCommit::new(CommitDescription::short_only("drop"), vec![]),
        ];

        let removed = retain_non_empty(&mut planned);

        assert_eq!(removed, 1);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].description.short, "keep");
    }
}
