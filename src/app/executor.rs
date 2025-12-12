use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use log::{debug, info, warn};

use crate::cancel;
use crate::editor::{Editor, EditorError};
use crate::git::{GitError, GitOps};
use crate::models::{BinaryFile, CommitDescription, Hunk, ModeChange, PlannedCommit};
use crate::patch::{FileMode, PatchContext};
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

    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        hunks: &[Hunk],
        planned_commits: &[PlannedCommit],
        new_files_to_commits: &HashMap<String, Vec<String>>,
        binary_files: &[BinaryFile],
        mode_changes: &[ModeChange],
        file_modes: &HashMap<PathBuf, FileMode>,
        no_verify: bool,
        no_editor: bool,
        plan: &mut SavedPlan,
    ) -> Result<(), ExecutionError> {
        let total = planned_commits.len();
        let start_index = plan.next_commit_index;

        // Create patch context from new_files_to_commits and file_modes
        // This tells PatchContext which files are NEW in the commit range
        // (didn't exist at base), and the file modes for proper patch headers.
        let patch_context = PatchContext::with_file_modes(new_files_to_commits.keys(), file_modes);

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
            let template = commit_message_template(&planned.description);
            let message = if no_editor {
                template
            } else {
                self.editor.edit(&template, &help_text)?
            };

            // Adjust hunk line numbers based on what's been applied to each file.
            // Note: Patch header generation (new/modified/deleted) is handled by
            // PatchContext which uses new_files_to_commits and index state.
            let adjusted_hunks =
                adjust_hunks_for_current_index(&commit_hunk_refs, &applied_hunks_per_file);

            // Skip this commit if all its changes were already applied in previous commits
            // AND there are no extra changes (binary files, mode changes) to apply
            let has_pending_extra_changes =
                !extra_changes_applied && (!binary_files.is_empty() || !mode_changes.is_empty());
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

            // Apply extra changes (binary files, mode-only changes) on the first non-skipped commit
            if !extra_changes_applied {
                if !binary_files.is_empty() {
                    debug!("Applying {} binary files...", binary_files.len());
                    self.git.apply_binary_files(binary_files)?;
                }
                // Mode changes for files WITH content hunks are handled via patch headers.
                // Here we only apply mode-only changes (files with no content hunks).
                let mode_only_changes: Vec<_> = mode_changes
                    .iter()
                    .filter(|mc| !hunks.iter().any(|h| h.file_path == mc.file_path))
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

/// Adjust hunk line numbers based on previously applied hunks.
///
/// When hunks are applied sequentially, later hunks need their line numbers
/// adjusted to account for lines added/removed by earlier hunks.
///
/// Note: Patch header generation (new/modified/deleted) is handled by `PatchContext`,
/// which uses `new_files_to_commits` and git index state. This function only adjusts
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

/// Apply mode-only patches (files with mode changes but no content hunks).
///
/// This generates a minimal patch for each mode change and applies it via git apply.
fn apply_mode_only_patches<G: GitOps>(
    git: &G,
    mode_changes: &[&ModeChange],
) -> Result<(), ExecutionError> {
    use crate::git::GitError;
    use std::io::Write;

    for mode_change in mode_changes {
        let path_str = mode_change.file_path.to_string_lossy();

        // Generate a mode-only patch
        let patch = format!(
            "diff --git a/{path} b/{path}\nold mode {old}\nnew mode {new}\n",
            path = path_str,
            old = mode_change.old_mode,
            new = mode_change.new_mode
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
