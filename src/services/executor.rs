use std::collections::{BTreeSet, HashMap, HashSet};

use crate::editor::Editor;
use crate::editor::EditorError;
use crate::git::{GitError, GitOps};
use crate::models::{CommitDescription, Hunk, PlannedCommit};
use crate::plan_store::{PlanFileError, PlanStore, SavedPlan};

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Editor(#[from] EditorError),
    #[error(transparent)]
    Plan(#[from] PlanFileError),
}

/// Applies planned commits by staging hunks, opening the editor, and committing.
pub struct PlanExecutor<'a, G: GitOps, E: Editor, P: PlanStore> {
    git: &'a G,
    editor: &'a E,
    plan_store: &'a P,
}

impl<'a, G: GitOps, E: Editor, P: PlanStore> PlanExecutor<'a, G, E, P> {
    pub fn new(git: &'a G, editor: &'a E, plan_store: &'a P) -> Self {
        Self {
            git,
            editor,
            plan_store,
        }
    }

    pub fn execute(
        &self,
        hunks: &[Hunk],
        planned_commits: &[PlannedCommit],
        new_files_to_commits: &HashMap<String, Vec<String>>,
        no_verify: bool,
        plan: &mut SavedPlan,
    ) -> Result<(), ExecutionError> {
        let total = planned_commits.len();
        let start_index = plan.next_commit_index;

        let mut staged_new_files: HashSet<String> = HashSet::new();
        for commit in planned_commits.iter().take(start_index) {
            let source_shas = collect_source_commits(commit, hunks);
            staged_new_files.extend(find_matching_new_files(new_files_to_commits, &source_shas));
        }

        for (i, planned) in planned_commits.iter().enumerate().skip(start_index) {
            println!("Creating commit {}/{}...", i + 1, total);

            let commit_hunk_refs: Vec<&Hunk> = planned
                .changes
                .iter()
                .filter_map(|change| change.resolve(hunks))
                .collect();

            let source_shas = collect_source_commits(planned, hunks);
            let new_files: Vec<&String> = new_files_to_commits
                .iter()
                .filter(|(f, cs)| {
                    !staged_new_files.contains(*f) && cs.iter().any(|c| source_shas.contains(c))
                })
                .map(|(f, _)| f)
                .collect();

            let help_text = generate_commit_help(&commit_hunk_refs, &new_files);
            let template = commit_message_template(&planned.description);
            let message = self.editor.edit(&template, &help_text)?;

            self.git.apply_hunks_to_index(&commit_hunk_refs)?;

            if !new_files.is_empty() {
                let paths: Vec<&std::path::Path> = new_files
                    .iter()
                    .map(|f| std::path::Path::new(f.as_str()))
                    .collect();
                self.git.stage_files(&paths)?;
                staged_new_files.extend(new_files.iter().map(|f| (*f).clone()));
            }

            let new_sha = self.git.commit(&message, no_verify)?;
            println!("  Created {}", short_sha(&new_sha));

            plan.mark_commit_created(new_sha);
            self.plan_store.save(plan)?;
        }

        Ok(())
    }
}

fn collect_source_commits(planned: &PlannedCommit, hunks: &[Hunk]) -> HashSet<String> {
    planned
        .changes
        .iter()
        .filter_map(|c| c.resolve(hunks))
        .flat_map(|h| h.likely_source_commits.clone())
        .collect()
}

fn find_matching_new_files(
    new_files_to_commits: &HashMap<String, Vec<String>>,
    source_shas: &HashSet<String>,
) -> Vec<String> {
    new_files_to_commits
        .iter()
        .filter(|(_, cs)| cs.iter().any(|c| source_shas.contains(c)))
        .map(|(f, _)| f.clone())
        .collect()
}

fn generate_commit_help(hunks: &[&Hunk], new_files: &[&String]) -> String {
    let files: BTreeSet<_> = hunks.iter().map(|h| &h.file_path).collect();
    let source_commits: BTreeSet<_> = hunks
        .iter()
        .flat_map(|h| &h.likely_source_commits)
        .collect();

    let mut lines = vec!["Files in this commit:".to_string()];
    lines.extend(files.iter().map(|f| format!("  {}", f.display())));

    if !new_files.is_empty() {
        lines.push(String::new());
        lines.push("New files:".to_string());
        lines.extend(new_files.iter().map(|f| format!("  {}", f)));
    }

    lines.push(String::new());
    lines.push(format!(
        "Total: {} hunks, {} files, {} new",
        hunks.len(),
        files.len(),
        new_files.len()
    ));

    if !source_commits.is_empty() {
        lines.push(String::new());
        lines.push("Source commits:".to_string());
        lines.extend(source_commits.iter().map(|s| format!("  {}", short_sha(s))));
    }

    lines.push(String::new());
    lines.push("Lines starting with '#' ignored. Empty message aborts.".to_string());

    lines.join("\n")
}

fn short_sha(sha: &str) -> &str {
    &sha[..8.min(sha.len())]
}

fn commit_message_template(desc: &CommitDescription) -> String {
    let short = desc.short.trim();
    let long = desc.long.trim();

    if long.is_empty() || long == short {
        return short.to_string();
    }

    if desc.long.starts_with(short) {
        return desc.long.clone();
    }

    format!("{}\n\n{}", short, long)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_short_only() {
        let desc = CommitDescription::short_only("fix bug");
        assert_eq!(commit_message_template(&desc), "fix bug");
    }

    #[test]
    fn template_short_and_body() {
        let desc = CommitDescription::new("feat", "add feature details");
        assert_eq!(
            commit_message_template(&desc),
            "feat\n\nadd feature details"
        );
    }

    #[test]
    fn template_when_long_contains_short() {
        let desc = CommitDescription::new("feat", "feat\n\nadd more");
        assert_eq!(commit_message_template(&desc), "feat\n\nadd more");
    }
}
