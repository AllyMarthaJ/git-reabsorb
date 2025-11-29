use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use crate::editor::{Editor, EditorError};
use crate::git::{GitError, GitOps};
use crate::models::{CommitDescription, DiffLine, Hunk, PlannedCommit};
use crate::plan_store::{PlanFileError, PlanStore, SavedPlan};
use crate::utils::short_sha;

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
        no_editor: bool,
        plan: &mut SavedPlan,
    ) -> Result<(), ExecutionError> {
        let total = planned_commits.len();
        let start_index = plan.next_commit_index;

        // Determine which files existed at HEAD (before unstaging)
        let mut files_at_head: HashSet<std::path::PathBuf> = HashSet::new();
        for hunk in hunks {
            if self.git.file_in_index(&hunk.file_path)? {
                files_at_head.insert(hunk.file_path.clone());
            }
        }

        let mut applied_hunks_per_file: HashMap<std::path::PathBuf, Vec<Hunk>> = HashMap::new();
        let mut created_new_files: HashSet<std::path::PathBuf> = HashSet::new();

        // Reconstruct applied hunks from previous commits
        for commit in planned_commits.iter().take(start_index) {
            for change in &commit.changes {
                if let Some(hunk) = change.resolve(hunks) {
                    // Track if we created a new file in this commit
                    if !files_at_head.contains(&hunk.file_path)
                        && !created_new_files.contains(&hunk.file_path)
                    {
                        created_new_files.insert(hunk.file_path.clone());
                    }

                    applied_hunks_per_file
                        .entry(hunk.file_path.clone())
                        .or_default()
                        .push(hunk.clone());
                }
            }
        }

        for (i, planned) in planned_commits.iter().enumerate().skip(start_index) {
            println!("Creating commit {}/{}...", i + 1, total);

            let commit_hunk_refs: Vec<&Hunk> = planned
                .changes
                .iter()
                .filter_map(|change| change.resolve(hunks))
                .collect();

            // Identify which files in this commit are genuinely new (didn't exist at HEAD)
            let new_file_paths: Vec<&std::path::PathBuf> = commit_hunk_refs
                .iter()
                .map(|h| &h.file_path)
                .filter(|f| !files_at_head.contains(*f) && !created_new_files.contains(*f))
                .collect();

            let help_text = generate_commit_help(&commit_hunk_refs, &new_file_paths);
            let template = commit_message_template(&planned.description);
            let message = if no_editor {
                template
            } else {
                self.editor.edit(&template, &help_text)?
            };

            // Adjust hunks based on what's been applied to each file
            let adjusted_hunks = adjust_hunks_for_current_index(
                &commit_hunk_refs,
                &applied_hunks_per_file,
                &files_at_head,
                &created_new_files,
            )?;

            // Skip this commit if all its changes were already applied in previous commits
            if adjusted_hunks.is_empty() {
                println!("  Skipped (all changes already applied)");
                plan.mark_commit_created("SKIPPED".to_string());
                self.plan_store.save(plan)?;
                continue;
            }

            let adjusted_refs: Vec<&Hunk> = adjusted_hunks.iter().collect();

            self.git.apply_hunks_to_index(&adjusted_refs)?;

            let new_sha = self.git.commit(&message, no_verify)?;
            println!("  Created {}", short_sha(&new_sha));

            // Track these hunks as applied for subsequent commits
            for hunk in commit_hunk_refs {
                // Track if we created a new file in this commit
                if !files_at_head.contains(&hunk.file_path)
                    && !created_new_files.contains(&hunk.file_path)
                {
                    created_new_files.insert(hunk.file_path.clone());
                }

                applied_hunks_per_file
                    .entry(hunk.file_path.clone())
                    .or_default()
                    .push(hunk.clone());
            }

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

fn generate_commit_help(hunks: &[&Hunk], new_files: &[&std::path::PathBuf]) -> String {
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
        lines.extend(new_files.iter().map(|f| format!("  {}", f.display())));
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

/// Adjust hunks based on what's been applied to each file.
/// Files that didn't exist at HEAD and haven't been created yet are converted to "new file" hunks.
/// Files that existed at HEAD or have been created have their old_start adjusted based on previously applied hunks.
fn adjust_hunks_for_current_index(
    hunks: &[&Hunk],
    applied_hunks_per_file: &HashMap<std::path::PathBuf, Vec<Hunk>>,
    files_at_head: &HashSet<std::path::PathBuf>,
    created_new_files: &HashSet<std::path::PathBuf>,
) -> Result<Vec<Hunk>, GitError> {
    let mut adjusted = Vec::new();

    // Group hunks by file
    let mut hunks_by_file: HashMap<&Path, Vec<&Hunk>> = HashMap::new();
    for hunk in hunks {
        hunks_by_file
            .entry(hunk.file_path.as_path())
            .or_default()
            .push(hunk);
    }

    for (file_path, file_hunks) in hunks_by_file {
        let file_path_buf = file_path.to_path_buf();

        // Check if this is a genuinely new file that hasn't been created yet
        let is_genuinely_new =
            !files_at_head.contains(&file_path_buf) && !created_new_files.contains(&file_path_buf);

        if is_genuinely_new {
            // File didn't exist at HEAD and we haven't created it yet - create a "new file" hunk
            let mut all_lines = Vec::new();

            for hunk in &file_hunks {
                for line in &hunk.lines {
                    // Convert all lines to additions
                    let content = match line {
                        DiffLine::Added(s) => s,
                        DiffLine::Context(s) => s,
                        DiffLine::Removed(s) => s,
                    };
                    all_lines.push(DiffLine::Added(content.clone()));
                }
            }

            let new_line_count = all_lines.len() as u32;

            adjusted.push(Hunk {
                id: file_hunks[0].id,
                file_path: file_path_buf,
                old_start: 0,
                old_count: 0,
                new_start: 1,
                new_count: new_line_count,
                lines: all_lines,
                likely_source_commits: file_hunks[0].likely_source_commits.clone(),
                old_missing_newline_at_eof: false,
                new_missing_newline_at_eof: file_hunks
                    .last()
                    .map(|h| h.new_missing_newline_at_eof)
                    .unwrap_or(false),
            });
        } else {
            // File existed at HEAD or has been created - adjust hunks based on previously applied changes
            let applied = applied_hunks_per_file
                .get(&file_path_buf)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);

            for hunk in file_hunks {
                let mut adjusted_hunk = (*hunk).clone();

                // Adjust old_start based on all previously applied hunks that came before this one
                for applied_hunk in applied {
                    if applied_hunk.old_start < hunk.old_start {
                        let delta =
                            (applied_hunk.new_count as i32) - (applied_hunk.old_count as i32);
                        adjusted_hunk.old_start = (adjusted_hunk.old_start as i32 + delta) as u32;
                    }
                }

                adjusted.push(adjusted_hunk);
            }
        }
    }

    Ok(adjusted)
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
