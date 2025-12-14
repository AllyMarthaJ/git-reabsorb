use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use log::{debug, info, warn};

use crate::cancel;
use crate::editor::{Editor, EditorError};
use crate::git::{GitError, GitOps};
use crate::models::{FileChange, Hunk, PlannedCommit};
use crate::patch::PatchContext;
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
    #[error("Cancelled by user")]
    Cancelled,
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
        file_changes: &[FileChange],
        no_verify: bool,
        no_editor: bool,
        plan: &mut SavedPlan,
    ) -> Result<(), ExecutionError> {
        let total = planned_commits.len();
        let start_index = plan.next_commit_index;

        let patch_context = PatchContext::new(file_changes);

        // Track which hunks have been applied (for line number adjustment)
        let mut applied_hunks_per_file: HashMap<std::path::PathBuf, Vec<Hunk>> = HashMap::new();

        // Track whether extra changes have been applied (only apply once)
        let mut extra_changes_applied = start_index > 0;

        // Reconstruct applied hunks from previous commits (for resumed execution)
        for commit in planned_commits.iter().take(start_index) {
            for change in &commit.changes {
                if let Some(hunk) = change.resolve(hunks) {
                    applied_hunks_per_file
                        .entry(hunk.file_path.clone())
                        .or_default()
                        .push(hunk.clone());
                }
            }
        }

        for (i, planned) in planned_commits.iter().enumerate().skip(start_index) {
            // Check for cancellation before each commit
            if cancel::is_cancelled() {
                warn!("Cancellation requested, stopping execution...");
                return Err(ExecutionError::Cancelled);
            }

            info!("Creating commit {}/{}...", i + 1, total);

            let commit_hunk_refs: Vec<&Hunk> = planned
                .changes
                .iter()
                .filter_map(|change| change.resolve(hunks))
                .collect();

            let help_text = generate_commit_help(&commit_hunk_refs);
            let template = planned.description.to_string();
            let message = if no_editor {
                template
            } else {
                self.editor.edit(&template, &help_text)?
            };

            // Adjust hunk line numbers based on what's been applied to each file.
            // Note: Patch header generation (new/modified/deleted) is handled by
            // PatchContext which uses file_changes and index state.
            let adjusted_hunks =
                adjust_hunks_for_current_index(&commit_hunk_refs, &applied_hunks_per_file);

            let has_pending_extra_changes = !extra_changes_applied && !file_changes.is_empty();
            if adjusted_hunks.is_empty() && !has_pending_extra_changes {
                debug!("Skipped (all changes already applied)");
                plan.mark_commit_created("SKIPPED".to_string());
                self.plan_store.save(plan)?;
                continue;
            }

            let adjusted_refs: Vec<&Hunk> = adjusted_hunks.iter().collect();

            if !adjusted_refs.is_empty() {
                self.git
                    .apply_hunks_to_index(&adjusted_refs, &patch_context)?;
            }

            if !extra_changes_applied {
                let binary_changes: Vec<_> =
                    file_changes.iter().filter(|fc| fc.is_binary).collect();
                if !binary_changes.is_empty() {
                    debug!("Applying {} binary files...", binary_changes.len());
                    self.git.apply_binary_files(&binary_changes)?;
                }
                let mode_only_changes: Vec<_> = file_changes
                    .iter()
                    .filter(|fc| !fc.has_content_hunks && !fc.is_binary)
                    .collect();
                if !mode_only_changes.is_empty() {
                    debug!("Applying {} mode-only changes...", mode_only_changes.len());
                    apply_mode_only_patches(self.git, &mode_only_changes)?;
                }
                extra_changes_applied = true;
            }

            let new_sha = self.git.commit(&message, no_verify)?;
            info!("Created {}", short_sha(&new_sha));

            // Track these hunks as applied for line number adjustment in subsequent commits
            for hunk in commit_hunk_refs {
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

fn generate_commit_help(hunks: &[&Hunk]) -> String {
    let files: BTreeSet<_> = hunks.iter().map(|h| &h.file_path).collect();
    let source_commits: BTreeSet<_> = hunks
        .iter()
        .flat_map(|h| &h.likely_source_commits)
        .collect();

    let mut lines = vec!["Files in this commit:".to_string()];
    lines.extend(files.iter().map(|f| format!("  {}", f.display())));

    lines.push(String::new());
    lines.push(format!(
        "Total: {} hunks, {} files",
        hunks.len(),
        files.len()
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

/// Adjust hunk line numbers based on previously applied hunks.
///
/// When hunks are applied sequentially, later hunks need their line numbers
/// adjusted to account for lines added/removed by earlier hunks.
///
/// Note: Patch header generation (new/modified/deleted) is handled by `PatchContext`,
/// which uses `file_changes` and git index state. This function only adjusts
/// line numbers for modifications to existing files.
fn adjust_hunks_for_current_index(
    hunks: &[&Hunk],
    applied_hunks_per_file: &HashMap<std::path::PathBuf, Vec<Hunk>>,
) -> Vec<Hunk> {
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

        // Get previously applied hunks for this file
        let applied = applied_hunks_per_file
            .get(&file_path_buf)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        for hunk in file_hunks {
            let mut adjusted_hunk = (*hunk).clone();

            // Adjust old_start based on all previously applied hunks that came before this one.
            // Each applied hunk shifts subsequent line numbers by (new_count - old_count).
            for applied_hunk in applied {
                if applied_hunk.old_start < hunk.old_start {
                    let delta = (applied_hunk.new_count as i32) - (applied_hunk.old_count as i32);
                    adjusted_hunk.old_start = (adjusted_hunk.old_start as i32 + delta) as u32;
                }
            }

            adjusted.push(adjusted_hunk);
        }
    }

    adjusted
}

fn apply_mode_only_patches<G: GitOps>(
    git: &G,
    file_changes: &[&FileChange],
) -> Result<(), ExecutionError> {
    use crate::git::GitError;
    use std::io::Write;

    for fc in file_changes {
        let (Some(old), Some(new)) = (&fc.old_mode, &fc.new_mode) else {
            continue;
        };

        let path_str = fc.file_path.to_string_lossy();

        // Generate a mode-only patch
        let patch = format!(
            "diff --git a/{path} b/{path}\nold mode {old}\nnew mode {new}\n",
            path = path_str,
            old = old,
            new = new
        );

        // Write to temp file and apply
        let mut temp_file = tempfile::NamedTempFile::new().map_err(GitError::ExecutionFailed)?;
        temp_file
            .write_all(patch.as_bytes())
            .map_err(GitError::ExecutionFailed)?;
        temp_file.flush().map_err(GitError::ExecutionFailed)?;

        git.run_git_output(&["apply", "--cached", temp_file.path().to_str().unwrap()])?;
    }

    Ok(())
}
