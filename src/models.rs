use std::path::PathBuf;

/// Unique identifier for a hunk
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HunkId(pub usize);

/// A commit read from the git history
#[derive(Debug, Clone)]
pub struct SourceCommit {
    pub sha: String,
    pub short_description: String,
    pub long_description: String,
}

/// A single line in a diff
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    /// Unchanged context line
    Context(String),
    /// Line added in this change
    Added(String),
    /// Line removed in this change
    Removed(String),
}

/// A hunk represents a contiguous region of changes in a file
#[derive(Debug, Clone)]
pub struct Hunk {
    pub id: HunkId,
    pub file_path: PathBuf,
    /// Starting line number in the original file
    pub old_start: u32,
    /// Number of lines in the original file
    pub old_count: u32,
    /// Starting line number in the new file
    pub new_start: u32,
    /// Number of lines in the new file
    pub new_count: u32,
    /// The diff lines (context, added, removed)
    pub lines: Vec<DiffLine>,
    /// SHA of the commit this hunk came from
    pub source_commit_sha: String,
}

impl Hunk {
    /// Convert this hunk to unified diff format
    pub fn to_patch(&self) -> String {
        let mut patch = String::new();

        // Hunk header
        patch.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            self.old_start, self.old_count, self.new_start, self.new_count
        ));

        // Diff lines
        for line in &self.lines {
            match line {
                DiffLine::Context(s) => {
                    patch.push(' ');
                    patch.push_str(s);
                    patch.push('\n');
                }
                DiffLine::Added(s) => {
                    patch.push('+');
                    patch.push_str(s);
                    patch.push('\n');
                }
                DiffLine::Removed(s) => {
                    patch.push('-');
                    patch.push_str(s);
                    patch.push('\n');
                }
            }
        }

        patch
    }
}

/// A commit description with short and long forms
#[derive(Debug, Clone)]
pub struct CommitDescription {
    /// First line of the commit message
    pub short: String,
    /// Full commit message (including the first line)
    pub long: String,
}

impl CommitDescription {
    pub fn new(short: impl Into<String>, long: impl Into<String>) -> Self {
        Self {
            short: short.into(),
            long: long.into(),
        }
    }

    /// Create from just a short description
    pub fn short_only(short: impl Into<String>) -> Self {
        let s = short.into();
        Self {
            short: s.clone(),
            long: s,
        }
    }
}

/// A planned commit - the output of reorganization
#[derive(Debug, Clone)]
pub struct PlannedCommit {
    pub description: CommitDescription,
    pub hunk_ids: Vec<HunkId>,
}

impl PlannedCommit {
    pub fn new(description: CommitDescription, hunk_ids: Vec<HunkId>) -> Self {
        Self {
            description,
            hunk_ids,
        }
    }
}
