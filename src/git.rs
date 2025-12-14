use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use crate::models::{Hunk, SourceCommit};
use crate::patch::parse;

/// Errors from git operations
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),
    #[error("Failed to execute git: {0}")]
    ExecutionFailed(#[from] std::io::Error),
    #[error("Failed to parse git output: {0}")]
    ParseError(String),
    #[error("Not a git repository")]
    NotARepository,
    #[error("No commits found in range {0}")]
    NoCommitsInRange(String),
    #[error("Failed to parse diff: {0}")]
    DiffParseError(#[from] crate::patch::ParseError),
    #[error("No pre-reabsorb state saved. Run 'git reabsorb plan' first.")]
    NoSavedState,
}

const PRE_REABSORB_REF_PREFIX: &str = "refs/reabsorb/pre-reabsorb";

/// Build the ref used to store the pre-reabsorb HEAD for a namespace
pub fn pre_reabsorb_ref_for(namespace: &str) -> String {
    format!("{}/{}", PRE_REABSORB_REF_PREFIX, namespace)
}

/// Trait for git operations - allows mocking in tests
pub trait GitOps {
    /// Find the merge-base between current HEAD and main/master (auto-detect)
    fn find_branch_base(&self) -> Result<String, GitError>;

    /// Find the merge-base between current HEAD and a specific branch
    fn find_merge_base(&self, branch: &str) -> Result<String, GitError>;

    /// Get the current HEAD SHA
    fn get_head(&self) -> Result<String, GitError>;

    /// Resolve a ref (branch name, tag, SHA prefix) to a full SHA
    fn resolve_ref(&self, ref_name: &str) -> Result<String, GitError>;

    /// Read commits in range (exclusive base, inclusive head)
    fn read_commits(&self, base: &str, head: &str) -> Result<Vec<SourceCommit>, GitError>;

    /// Read hunks from a commit's diff against its parent
    fn read_hunks(&self, commit_sha: &str, hunk_id_start: usize) -> Result<Vec<Hunk>, GitError>;

    /// Get the raw diff output between HEAD and working tree
    fn get_working_tree_diff(&self) -> Result<String, GitError>;

    /// Get diff between two tree-ish references
    fn diff_trees(&self, left: &str, right: &str) -> Result<String, GitError>;

    /// Get diff for a specific file between index and working tree
    fn diff_file_in_working_tree(&self, file_path: &str) -> Result<String, GitError>;

    /// Get list of files changed in a specific commit
    fn get_files_changed_in_commit(&self, commit_sha: &str) -> Result<Vec<String>, GitError>;

    /// Get list of newly added files in a specific commit (files that didn't exist before)
    fn get_new_files_in_commit(&self, commit_sha: &str) -> Result<Vec<String>, GitError>;

    /// Apply a single hunk to the index using git apply
    fn apply_hunk_to_index(&self, hunk: &Hunk) -> Result<(), GitError>;

    /// Reset to a ref (mixed reset - unstages to working tree)
    fn reset_to(&self, ref_name: &str) -> Result<(), GitError>;

    /// Hard reset to a ref (discards all changes)
    fn reset_hard(&self, ref_name: &str) -> Result<(), GitError>;

    /// Apply hunks to the index (stage them)
    ///
    /// The `patch_context` provides information about which files are new in the
    /// commit range, enabling correct patch header generation.
    fn apply_hunks_to_index(
        &self,
        hunks: &[&Hunk],
        patch_context: &crate::patch::PatchContext,
    ) -> Result<(), GitError>;

    /// Stage all changes in the working tree (git add -A)
    fn stage_all(&self) -> Result<(), GitError>;

    /// Stage specific files (git add <files>)
    fn stage_files(&self, files: &[&Path]) -> Result<(), GitError>;

    /// Create a commit with the currently staged changes
    fn commit(&self, message: &str, no_verify: bool) -> Result<String, GitError>;

    /// Save the current HEAD as the pre-reabsorb state
    fn save_pre_reabsorb_head(&self, ref_name: &str) -> Result<(), GitError>;

    /// Get the saved pre-reabsorb HEAD, if any
    fn get_pre_reabsorb_head(&self, ref_name: &str) -> Result<String, GitError>;

    /// Check if a pre-reabsorb state is saved
    fn has_pre_reabsorb_head(&self, ref_name: &str) -> bool;

    /// Clear the saved pre-reabsorb state
    fn clear_pre_reabsorb_head(&self, ref_name: &str) -> Result<(), GitError>;

    /// Get the current branch name ("HEAD" if detached)
    fn current_branch_name(&self) -> Result<String, GitError>;

    /// Check if a file exists in the git index
    fn file_in_index(&self, file_path: &Path) -> Result<bool, GitError>;

    /// Run a git command and return the output (for debugging)
    fn run_git_output(&self, args: &[&str]) -> Result<String, GitError>;

    /// Apply binary file changes to the index.
    fn apply_binary_files(&self, changes: &[&crate::models::FileChange]) -> Result<(), GitError>;
}

/// Real implementation of GitOps that calls git commands
pub struct Git {
    /// Working directory for git commands
    work_dir: Option<std::path::PathBuf>,
}

impl Git {
    pub fn new() -> Self {
        Self { work_dir: None }
    }

    pub fn with_repo_root() -> Result<Self, GitError> {
        let repo_root = Self::find_repo_root(".")?;
        Ok(Self {
            work_dir: Some(std::path::PathBuf::from(repo_root)),
        })
    }

    fn find_repo_root(work_dir: impl AsRef<Path>) -> Result<String, GitError> {
        let mut cmd = Command::new("git");
        cmd.current_dir(work_dir.as_ref());
        cmd.args(["rev-parse", "--show-toplevel"]);

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::CommandFailed(format!(
                "git rev-parse --show-toplevel failed: {}",
                stderr
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn with_work_dir(work_dir: impl AsRef<Path>) -> Self {
        Self {
            work_dir: Some(work_dir.as_ref().to_path_buf()),
        }
    }

    fn run_git(&self, args: &[&str]) -> Result<String, GitError> {
        let mut cmd = Command::new("git");
        if let Some(ref dir) = self.work_dir {
            cmd.current_dir(dir);
        }
        cmd.args(args);

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::CommandFailed(format!(
                "git {} failed: {}",
                args.join(" "),
                stderr
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

impl Default for Git {
    fn default() -> Self {
        Self::new()
    }
}

impl GitOps for Git {
    fn find_branch_base(&self) -> Result<String, GitError> {
        // Try to find merge-base with main first, then master
        for base_branch in &["main", "master"] {
            let result = self.run_git(&["merge-base", base_branch, "HEAD"]);
            if let Ok(sha) = result {
                return Ok(sha.trim().to_string());
            }
        }

        // If neither exists, return an error
        Err(GitError::CommandFailed(
            "Could not find merge-base with main or master".to_string(),
        ))
    }

    fn find_merge_base(&self, branch: &str) -> Result<String, GitError> {
        let output = self.run_git(&["merge-base", branch, "HEAD"])?;
        Ok(output.trim().to_string())
    }

    fn get_head(&self) -> Result<String, GitError> {
        let output = self.run_git(&["rev-parse", "HEAD"])?;
        Ok(output.trim().to_string())
    }

    fn resolve_ref(&self, ref_name: &str) -> Result<String, GitError> {
        let output = self.run_git(&["rev-parse", ref_name])?;
        Ok(output.trim().to_string())
    }

    fn read_commits(&self, base: &str, head: &str) -> Result<Vec<SourceCommit>, GitError> {
        // Get commit SHAs in range (oldest first)
        // Note: base..head is exclusive of base (merge-base is not included)
        let range = format!("{}..{}", base, head);
        let output = self.run_git(&["rev-list", "--reverse", &range])?;

        let shas: Vec<&str> = output.lines().filter(|s| !s.is_empty()).collect();
        if shas.is_empty() {
            return Err(GitError::NoCommitsInRange(range));
        }

        let mut commits = Vec::new();
        for sha in shas {
            // Get full commit message
            let message = self.run_git(&["log", "-1", "--format=%B", sha])?;
            let message = message.trim();
            let short = message.lines().next().unwrap_or("").to_string();

            commits.push(SourceCommit::new(sha, short, message));
        }

        Ok(commits)
    }

    fn read_hunks(&self, commit_sha: &str, hunk_id_start: usize) -> Result<Vec<Hunk>, GitError> {
        // Get diff for this commit against its parent
        let diff_output = self.run_git(&["show", "--format=", "-p", commit_sha])?;

        let hunks = parse(&diff_output, &[commit_sha.to_string()], hunk_id_start)?.hunks;
        Ok(hunks)
    }

    fn get_working_tree_diff(&self) -> Result<String, GitError> {
        // Disable rename detection to get explicit deletion and creation hunks
        // This ensures renamed files are handled as delete + create, not just modify
        let output = self.run_git(&["diff", "HEAD", "--no-color", "--no-renames"])?;
        Ok(output)
    }

    fn diff_trees(&self, left: &str, right: &str) -> Result<String, GitError> {
        // Disable rename detection to get explicit deletion and creation hunks
        let output = self.run_git(&["diff", left, right, "--no-color", "--no-renames"])?;
        Ok(output)
    }

    fn diff_file_in_working_tree(&self, file_path: &str) -> Result<String, GitError> {
        let output = self.run_git(&["diff", "--no-color", "--", file_path])?;
        Ok(output)
    }

    fn get_files_changed_in_commit(&self, commit_sha: &str) -> Result<Vec<String>, GitError> {
        let output = self.run_git(&[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            commit_sha,
        ])?;
        Ok(output.lines().map(|s| s.to_string()).collect())
    }

    fn get_new_files_in_commit(&self, commit_sha: &str) -> Result<Vec<String>, GitError> {
        // Use --name-status to get status codes (A = added, M = modified, D = deleted)
        let output = self.run_git(&[
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "-r",
            commit_sha,
        ])?;

        // Filter for lines starting with "A\t" (added files)
        let new_files = output
            .lines()
            .filter_map(|line| line.strip_prefix("A\t").map(String::from))
            .collect();

        Ok(new_files)
    }

    fn apply_hunk_to_index(&self, hunk: &Hunk) -> Result<(), GitError> {
        let patch = hunk.to_full_patch();

        // Write patch to temp file
        let mut temp_file = tempfile::NamedTempFile::new()?;
        temp_file.write_all(patch.as_bytes())?;
        temp_file.flush()?;

        // Apply patch to index
        self.run_git(&[
            "apply",
            "--cached",
            "--unidiff-zero",
            temp_file.path().to_str().unwrap(),
        ])?;

        Ok(())
    }

    fn reset_to(&self, ref_name: &str) -> Result<(), GitError> {
        self.run_git(&["reset", ref_name])?;
        Ok(())
    }

    fn reset_hard(&self, ref_name: &str) -> Result<(), GitError> {
        self.run_git(&["reset", "--hard", ref_name])?;
        Ok(())
    }

    fn apply_hunks_to_index(
        &self,
        hunks: &[&Hunk],
        patch_context: &crate::patch::PatchContext,
    ) -> Result<(), GitError> {
        if hunks.is_empty() {
            return Ok(());
        }

        // Group hunks by file
        let mut hunks_by_file: HashMap<std::path::PathBuf, Vec<&Hunk>> = HashMap::new();
        for hunk in hunks {
            hunks_by_file
                .entry(hunk.file_path.clone())
                .or_default()
                .push(hunk);
        }

        // For each file, use PatchContext to generate correct patch and apply it
        for (file_path, mut file_hunks) in hunks_by_file {
            let file_path = file_path.as_path();
            // Sort hunks by line number - git expects hunks in order
            file_hunks.sort_by_key(|h| h.old_start);

            // Check actual git index state
            let file_in_index = self.file_in_index(file_path)?;

            // Use PatchContext to generate the patch with correct headers
            let (patch, _change_type) =
                patch_context.generate_patch(file_path, &file_hunks, file_in_index);

            if patch.is_empty() {
                continue;
            }

            // Write patch to temp file and apply
            let mut temp_file = tempfile::NamedTempFile::new()?;
            temp_file.write_all(patch.as_bytes())?;
            temp_file.flush()?;

            // Apply patch to index
            self.run_git(&[
                "apply",
                "--cached",
                "--unidiff-zero",
                temp_file.path().to_str().unwrap(),
            ])?;
        }

        Ok(())
    }

    fn stage_all(&self) -> Result<(), GitError> {
        self.run_git(&["add", "-A"])?;
        Ok(())
    }

    fn stage_files(&self, files: &[&Path]) -> Result<(), GitError> {
        if files.is_empty() {
            return Ok(());
        }

        let mut args = vec!["add", "--"];
        for file in files {
            args.push(file.to_str().unwrap());
        }
        self.run_git(&args)?;
        Ok(())
    }

    fn commit(&self, message: &str, no_verify: bool) -> Result<String, GitError> {
        // Write message to temp file to handle multiline messages
        let mut temp_file = tempfile::NamedTempFile::new()?;
        temp_file.write_all(message.as_bytes())?;
        temp_file.flush()?;

        let mut args = vec!["commit", "-F", temp_file.path().to_str().unwrap()];
        if no_verify {
            args.push("--no-verify");
        }
        self.run_git(&args)?;

        // Get the new commit SHA
        self.get_head()
    }

    fn save_pre_reabsorb_head(&self, ref_name: &str) -> Result<(), GitError> {
        let head = self.get_head()?;
        self.run_git(&["update-ref", ref_name, &head])?;
        Ok(())
    }

    fn get_pre_reabsorb_head(&self, ref_name: &str) -> Result<String, GitError> {
        let result = self.run_git(&["rev-parse", ref_name]);
        match result {
            Ok(sha) => Ok(sha.trim().to_string()),
            Err(_) => Err(GitError::NoSavedState),
        }
    }

    fn has_pre_reabsorb_head(&self, ref_name: &str) -> bool {
        self.run_git(&["rev-parse", "--verify", ref_name]).is_ok()
    }

    fn clear_pre_reabsorb_head(&self, ref_name: &str) -> Result<(), GitError> {
        if self.has_pre_reabsorb_head(ref_name) {
            self.run_git(&["update-ref", "-d", ref_name])?;
        }
        Ok(())
    }

    fn current_branch_name(&self) -> Result<String, GitError> {
        let output = self.run_git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
        Ok(output.trim().to_string())
    }

    fn file_in_index(&self, file_path: &Path) -> Result<bool, GitError> {
        let path_str = file_path.to_str().unwrap();

        // Check with ls-files
        let result = self.run_git(&["ls-files", "--", path_str]);
        if let Ok(output) = &result {
            if !output.trim().is_empty() {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn run_git_output(&self, args: &[&str]) -> Result<String, GitError> {
        self.run_git(args)
    }

    fn apply_binary_files(&self, changes: &[&crate::models::FileChange]) -> Result<(), GitError> {
        use crate::models::ChangeType;

        let binary_changes: Vec<_> = changes.iter().filter(|fc| fc.is_binary).collect();

        for fc in binary_changes {
            let path_str = fc.file_path.to_str().unwrap();

            match fc.change_type {
                ChangeType::Added | ChangeType::Modified => {
                    self.run_git(&["add", "--", path_str])?;
                }
                ChangeType::Deleted => {
                    self.run_git(&["rm", "--cached", "--", path_str])?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ChangeType, DiffLine, FileChange, HunkId};
    use crate::patch::PatchContext;
    use std::path::PathBuf;

    fn make_modification_hunk() -> Hunk {
        Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("test.rs"),
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 4,
            lines: vec![
                DiffLine::Context("fn main() {".to_string()),
                DiffLine::Added("    println!(\"Hello\");".to_string()),
                DiffLine::Context("    println!(\"World\");".to_string()),
                DiffLine::Context("}".to_string()),
            ],
            likely_source_commits: vec!["abc123".to_string()],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        }
    }

    #[test]
    fn test_patch_for_existing_file() {
        let hunk = make_modification_hunk();
        let ctx = PatchContext::empty();
        // File exists in index -> modification patch
        let (patch, _) = ctx.generate_patch(Path::new("test.rs"), &[&hunk], true);
        assert!(patch.contains("--- a/test.rs"), "Should have old path");
        assert!(patch.contains("+++ b/test.rs"), "Should have new path");
        assert!(patch.contains("@@ -1,3 +1,4 @@"));
    }

    #[test]
    fn test_patch_for_new_file() {
        let hunk = make_modification_hunk();
        let ctx = PatchContext::empty();
        // File does NOT exist in index -> new file patch (transformed)
        let (patch, _) = ctx.generate_patch(Path::new("test.rs"), &[&hunk], false);
        assert!(patch.contains("--- /dev/null"), "Should be new file");
        assert!(patch.contains("+++ b/test.rs"));
    }

    #[test]
    fn test_patch_for_new_file_in_range() {
        let hunk = make_modification_hunk();
        // Mark file as new in range - even with modification hunks,
        // it should generate a new file patch
        let file_changes = vec![FileChange {
            file_path: PathBuf::from("test.rs"),
            change_type: ChangeType::Added,
            old_mode: None,
            new_mode: Some("100644".to_string()),
            is_binary: false,
            has_content_hunks: true,
            likely_source_commits: vec![],
        }];
        let ctx = PatchContext::new(&file_changes);
        let (patch, _) = ctx.generate_patch(Path::new("test.rs"), &[&hunk], false);
        assert!(patch.contains("--- /dev/null"), "Should be new file");
        assert!(patch.contains("+++ b/test.rs"));
    }

    #[test]
    fn test_patch_for_deletion() {
        let hunk = Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("test.rs"),
            old_start: 1,
            old_count: 3,
            new_start: 0,
            new_count: 0,
            lines: vec![
                DiffLine::Removed("line1".to_string()),
                DiffLine::Removed("line2".to_string()),
                DiffLine::Removed("line3".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };

        let ctx = PatchContext::empty();
        // File exists and all hunks are deletions -> delete patch
        let (patch, _) = ctx.generate_patch(Path::new("test.rs"), &[&hunk], true);
        assert!(patch.contains("--- a/test.rs"));
        assert!(patch.contains("+++ /dev/null"), "Should delete file");
    }
}
