use std::collections::HashMap;

use log::debug;

use crate::git::{GitError, GitOps};
use crate::patch::{parse, ParseError, Patch};
use crate::models::{FileChange, Hunk, PlannedCommit, SourceCommit, Strategy};
use crate::reorganize::ReorganizeError;

use super::StrategyFactory;

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

        Ok(PlanDraft {
            strategy,
            planned_commits,
            hunks: hunks.to_vec(),
            file_to_commits: file_to_commits.clone(),
            file_changes: file_changes.to_vec(),
        })
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
