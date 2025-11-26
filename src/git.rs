use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use crate::diff_parser::parse_diff;
use crate::models::{Hunk, SourceCommit};

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
    DiffParseError(#[from] crate::diff_parser::DiffParseError),
}

/// Trait for git operations - allows mocking in tests
pub trait GitOps {
    /// Find the merge-base between current HEAD and main/master
    fn find_branch_base(&self) -> Result<String, GitError>;

    /// Get the current HEAD SHA
    fn get_head(&self) -> Result<String, GitError>;

    /// Resolve a ref (branch name, tag, SHA prefix) to a full SHA
    fn resolve_ref(&self, ref_name: &str) -> Result<String, GitError>;

    /// Read commits in range (exclusive base, inclusive head)
    fn read_commits(&self, base: &str, head: &str) -> Result<Vec<SourceCommit>, GitError>;

    /// Read hunks from a commit's diff against its parent
    fn read_hunks(&self, commit_sha: &str, hunk_id_start: usize) -> Result<Vec<Hunk>, GitError>;

    /// Reset to a ref (mixed reset - unstages to working tree)
    fn reset_to(&self, ref_name: &str) -> Result<(), GitError>;

    /// Hard reset to a ref (discards all changes)
    fn reset_hard(&self, ref_name: &str) -> Result<(), GitError>;

    /// Apply hunks to the index (stage them)
    fn apply_hunks_to_index(&self, hunks: &[&Hunk]) -> Result<(), GitError>;

    /// Create a commit with the currently staged changes
    fn commit(&self, message: &str) -> Result<String, GitError>;
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

            let short_description = message.lines().next().unwrap_or("").to_string();
            let long_description = message.to_string();

            commits.push(SourceCommit {
                sha: sha.to_string(),
                short_description,
                long_description,
            });
        }

        Ok(commits)
    }

    fn read_hunks(&self, commit_sha: &str, hunk_id_start: usize) -> Result<Vec<Hunk>, GitError> {
        // Get diff for this commit against its parent
        let diff_output = self.run_git(&["show", "--format=", "-p", commit_sha])?;

        let hunks = parse_diff(&diff_output, commit_sha, hunk_id_start)?;
        Ok(hunks)
    }

    fn reset_to(&self, ref_name: &str) -> Result<(), GitError> {
        self.run_git(&["reset", ref_name])?;
        Ok(())
    }

    fn reset_hard(&self, ref_name: &str) -> Result<(), GitError> {
        self.run_git(&["reset", "--hard", ref_name])?;
        Ok(())
    }

    fn apply_hunks_to_index(&self, hunks: &[&Hunk]) -> Result<(), GitError> {
        if hunks.is_empty() {
            return Ok(());
        }

        // Group hunks by file
        let mut hunks_by_file: HashMap<&Path, Vec<&Hunk>> = HashMap::new();
        for hunk in hunks {
            hunks_by_file
                .entry(hunk.file_path.as_path())
                .or_default()
                .push(hunk);
        }

        // For each file, create a patch and apply it
        for (file_path, file_hunks) in hunks_by_file {
            let patch = create_patch_for_file(file_path, &file_hunks);

            // Write patch to temp file and apply
            let mut temp_file = tempfile::NamedTempFile::new()?;
            temp_file.write_all(patch.as_bytes())?;
            temp_file.flush()?;

            // Apply patch to index
            let result = self.run_git(&[
                "apply",
                "--cached",
                "--unidiff-zero",
                temp_file.path().to_str().unwrap(),
            ]);

            if let Err(e) = result {
                // If applying to index fails, try applying to working tree first then staging
                eprintln!(
                    "Warning: direct index apply failed for {}, trying alternative method: {}",
                    file_path.display(),
                    e
                );

                // Stage the whole file instead
                self.run_git(&["add", file_path.to_str().unwrap()])?;
            }
        }

        Ok(())
    }

    fn commit(&self, message: &str) -> Result<String, GitError> {
        // Write message to temp file to handle multiline messages
        let mut temp_file = tempfile::NamedTempFile::new()?;
        temp_file.write_all(message.as_bytes())?;
        temp_file.flush()?;

        self.run_git(&["commit", "-F", temp_file.path().to_str().unwrap()])?;

        // Get the new commit SHA
        self.get_head()
    }
}

/// Create a unified diff patch for a single file from multiple hunks
fn create_patch_for_file(file_path: &Path, hunks: &[&Hunk]) -> String {
    let mut patch = String::new();

    let path_str = file_path.to_string_lossy();

    // Patch header
    patch.push_str(&format!("--- a/{}\n", path_str));
    patch.push_str(&format!("+++ b/{}\n", path_str));

    // Add each hunk
    for hunk in hunks {
        patch.push_str(&hunk.to_patch());
    }

    patch
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DiffLine, HunkId};
    use std::path::PathBuf;

    #[test]
    fn test_create_patch_for_file() {
        let hunk = Hunk {
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
            source_commit_sha: "abc123".to_string(),
        };

        let patch = create_patch_for_file(Path::new("test.rs"), &[&hunk]);
        assert!(patch.contains("--- a/test.rs"));
        assert!(patch.contains("+++ b/test.rs"));
        assert!(patch.contains("@@ -1,3 +1,4 @@"));
    }
}
