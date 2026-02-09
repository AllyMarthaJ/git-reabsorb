use std::{
    fmt::{Display, Formatter},
    path::PathBuf,
};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// Reorganization strategy
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Strategy {
    /// Preserve original commit structure
    Preserve,
    /// Group changes by file (one commit per file)
    #[value(name = "by-file")]
    ByFile,
    /// Squash all changes into a single commit
    Squash,
    /// Use LLM to intelligently reorganize commits (single-shot)
    Llm,
    /// Multi-phase hierarchical reorganization (scales to large changes)
    Hierarchical,
    /// Use git-absorb to fixup commits
    Absorb,
}

/// Unique identifier for a hunk within a reabsorb operation
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HunkId(pub usize);

impl std::fmt::Display for HunkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "hunk#{}", self.0)
    }
}

/// Unique identifier for a planned commit
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlannedCommitId(pub usize);

impl std::fmt::Display for PlannedCommitId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "commit#{}", self.0)
    }
}

/// A commit read from the git history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCommit {
    pub sha: String,
    pub message: CommitDescription,
}

impl SourceCommit {
    pub fn new(sha: impl Into<String>, short: impl Into<String>, long: impl Into<String>) -> Self {
        Self {
            sha: sha.into(),
            message: CommitDescription::new(short, long),
        }
    }
}

/// The type of change to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
}

/// A file change tracking mode, binary status, and change type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    #[serde(with = "path_serde")]
    pub file_path: PathBuf,
    pub change_type: ChangeType,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    #[serde(default)]
    pub is_binary: bool,
    #[serde(default)]
    pub has_content_hunks: bool,
    pub likely_source_commits: Vec<String>,
}

impl FileChange {
    pub fn with_path(path: PathBuf) -> Self {
        Self {
            file_path: path,
            change_type: ChangeType::Modified,
            old_mode: None,
            new_mode: None,
            is_binary: false,
            has_content_hunks: false,
            likely_source_commits: vec![],
        }
    }
}

/// A single line in a diff
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum DiffLine {
    /// Unchanged context line
    #[serde(rename = "context")]
    Context(String),
    /// Line added in this change
    #[serde(rename = "added")]
    Added(String),
    /// Line removed in this change
    #[serde(rename = "removed")]
    Removed(String),
}

/// A hunk represents a contiguous region of changes in a file.
///
/// These hunks are parsed from the unified diff between base and final state,
/// so all line numbers are relative to the same base. This means hunks can be
/// applied independently (as long as they don't overlap).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    pub id: HunkId,
    #[serde(with = "path_serde")]
    pub file_path: PathBuf,
    /// Starting line number in the base (original) file
    pub old_start: u32,
    /// Number of lines in the base file
    pub old_count: u32,
    /// Starting line number in the new (final) file
    pub new_start: u32,
    /// Number of lines in the new file
    pub new_count: u32,
    /// The diff lines (context, added, removed)
    pub lines: Vec<DiffLine>,
    /// Source commits that likely contributed to this hunk.
    /// Determined by matching file paths and analyzing which commits
    /// touched the same regions of the file.
    pub likely_source_commits: Vec<String>,
    /// True if the old file is missing a newline at EOF
    #[serde(default)]
    pub old_missing_newline_at_eof: bool,
    /// True if the new file is missing a newline at EOF
    #[serde(default)]
    pub new_missing_newline_at_eof: bool,
}

mod path_serde {
    use serde::{self, Deserialize, Deserializer, Serializer};
    use std::path::{Path, PathBuf};

    pub fn serialize<S>(path: &Path, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&path.to_string_lossy())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PathBuf, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(PathBuf::from(s))
    }
}

impl Hunk {
    /// Convert this hunk to unified diff format suitable for `git apply`.
    ///
    /// This generates just the hunk body (header + lines), without file headers.
    /// For a complete patch with file headers, use `to_full_patch()`.
    #[must_use]
    pub fn to_patch(&self) -> String {
        crate::patch::PatchWriter::write_hunk_body(self)
    }

    /// Generate a full patch for this hunk (with file headers).
    ///
    /// This generates a complete unified diff patch suitable for `git apply`.
    /// The file change type (new/modified/deleted) is inferred from the hunk's
    /// line counts.
    #[must_use]
    pub fn to_full_patch(&self) -> String {
        crate::patch::PatchWriter::write_single_hunk(self)
    }
}

/// A commit description with short and long forms
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommitDescription {
    /// First line of the commit message
    #[serde(alias = "short_description")]
    pub short: String,
    /// Full commit message (including the first line)
    #[serde(alias = "long_description")]
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

impl Display for CommitDescription {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        let short = self.short.trim();
        let long = self.long.trim();

        if long.is_empty() || long == short {
            return write!(f, "{}", short);
        }

        if self.long.starts_with(short) {
            return write!(f, "{}", self.long);
        }

        write!(f, "{}\n\n{}", short, long)
    }
}

/// A change to include in a planned commit
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum PlannedChange {
    /// Reference to an existing hunk by ID
    #[serde(rename = "existing")]
    ExistingHunk(HunkId),
    /// A new hunk (from splitting/merging/LLM generation)
    #[serde(rename = "new")]
    NewHunk(Hunk),
}

impl PlannedChange {
    /// Resolve this change to a concrete Hunk
    #[must_use]
    pub fn resolve<'a>(&'a self, hunks: &'a [Hunk]) -> Option<&'a Hunk> {
        match self {
            PlannedChange::ExistingHunk(id) => hunks.iter().find(|h| h.id == *id),
            PlannedChange::NewHunk(hunk) => Some(hunk),
        }
    }
}

/// A planned commit - the output of reorganization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedCommit {
    /// Unique identifier for this planned commit
    pub id: PlannedCommitId,
    pub description: CommitDescription,
    pub changes: Vec<PlannedChange>,
    /// Other planned commits this depends on (must be committed first)
    #[serde(default)]
    pub depends_on: Vec<PlannedCommitId>,
}

impl PlannedCommit {
    pub fn new(
        id: PlannedCommitId,
        description: CommitDescription,
        changes: Vec<PlannedChange>,
    ) -> Self {
        Self {
            id,
            description,
            changes,
            depends_on: Vec::new(),
        }
    }

    /// Create with dependencies
    pub fn with_dependencies(
        id: PlannedCommitId,
        description: CommitDescription,
        changes: Vec<PlannedChange>,
        depends_on: Vec<PlannedCommitId>,
    ) -> Self {
        Self {
            id,
            description,
            changes,
            depends_on,
        }
    }

    /// Create a PlannedCommit from hunk IDs (convenience for existing reorganizers)
    pub fn from_hunk_ids(
        id: PlannedCommitId,
        description: CommitDescription,
        hunk_ids: Vec<HunkId>,
    ) -> Self {
        Self {
            id,
            description,
            changes: hunk_ids
                .into_iter()
                .map(PlannedChange::ExistingHunk)
                .collect(),
            depends_on: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_hunk() -> Hunk {
        Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("src/main.rs"),
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
    fn test_hunk_to_patch() {
        let hunk = make_test_hunk();
        let patch = hunk.to_patch();

        // Should contain hunk header
        assert!(patch.contains("@@ -1,3 +1,4 @@"));
        // Should contain context line with space prefix
        assert!(patch.contains(" fn main() {"));
        // Should contain added line with + prefix
        assert!(patch.contains("+    println!(\"Hello\");"));
    }

    #[test]
    fn test_hunk_to_full_patch() {
        let hunk = make_test_hunk();
        let patch = hunk.to_full_patch();

        // Should contain file headers
        assert!(patch.contains("--- a/src/main.rs"));
        assert!(patch.contains("+++ b/src/main.rs"));
        // Should also contain the hunk content
        assert!(patch.contains("@@ -1,3 +1,4 @@"));
    }

    #[test]
    fn test_hunk_to_full_patch_deleted_file() {
        let hunk = Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("src/old.rs"),
            old_start: 1,
            old_count: 3,
            new_start: 0,
            new_count: 0,
            lines: vec![
                DiffLine::Removed("fn old() {".to_string()),
                DiffLine::Removed("    // deleted".to_string()),
                DiffLine::Removed("}".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };
        let patch = hunk.to_full_patch();

        // For deleted files, should use /dev/null as new path
        assert!(patch.contains("--- a/src/old.rs"), "Patch: {}", patch);
        assert!(patch.contains("+++ /dev/null"), "Patch: {}", patch);
        assert!(patch.contains("@@ -1,3 +0,0 @@"));
    }

    #[test]
    fn test_hunk_to_full_patch_new_file() {
        let hunk = Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("src/new.rs"),
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: 3,
            lines: vec![
                DiffLine::Added("fn new() {".to_string()),
                DiffLine::Added("    // new".to_string()),
                DiffLine::Added("}".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };
        let patch = hunk.to_full_patch();

        // For new files, should use /dev/null as old path
        assert!(patch.contains("--- /dev/null"), "Patch: {}", patch);
        assert!(patch.contains("+++ b/src/new.rs"), "Patch: {}", patch);
        assert!(patch.contains("@@ -0,0 +1,3 @@"));
    }

    #[test]
    fn test_hunk_to_patch_with_removed_lines() {
        let hunk = Hunk {
            id: HunkId(1),
            file_path: PathBuf::from("test.rs"),
            old_start: 5,
            old_count: 4,
            new_start: 5,
            new_count: 3,
            lines: vec![
                DiffLine::Context("let x = 1;".to_string()),
                DiffLine::Removed("let y = 2;".to_string()),
                DiffLine::Context("let z = 3;".to_string()),
                DiffLine::Context("return x + z;".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };
        let patch = hunk.to_patch();

        assert!(patch.contains("@@ -5,4 +5,3 @@"));
        assert!(patch.contains("-let y = 2;"));
        assert!(patch.contains(" let x = 1;"));
    }

    #[test]
    fn test_hunk_to_patch_all_added() {
        let hunk = Hunk {
            id: HunkId(2),
            file_path: PathBuf::from("new_file.rs"),
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: 2,
            lines: vec![
                DiffLine::Added("fn new() {}".to_string()),
                DiffLine::Added("".to_string()),
            ],
            likely_source_commits: vec!["def456".to_string()],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };
        let patch = hunk.to_patch();

        assert!(patch.contains("@@ -0,0 +1,2 @@"));
        assert!(patch.contains("+fn new() {}"));
    }

    #[test]
    fn test_commit_description_new() {
        let desc = CommitDescription::new("Short message", "Long message\n\nWith details");
        assert_eq!(desc.short, "Short message");
        assert_eq!(desc.long, "Long message\n\nWith details");
    }

    #[test]
    fn test_commit_description_short_only() {
        let desc = CommitDescription::short_only("Just a short message");
        assert_eq!(desc.short, "Just a short message");
        assert_eq!(desc.long, "Just a short message");
    }

    #[test]
    fn test_commit_description_to_message_short_only() {
        let desc = CommitDescription::short_only("fix bug");
        assert_eq!(desc.to_string(), "fix bug");
    }

    #[test]
    fn test_commit_description_to_message_short_and_body() {
        let desc = CommitDescription::new("feat", "add feature details");
        assert_eq!(desc.to_string(), "feat\n\nadd feature details");
    }

    #[test]
    fn test_commit_description_to_message_long_contains_short() {
        let desc = CommitDescription::new("feat", "feat\n\nadd more");
        assert_eq!(desc.to_string(), "feat\n\nadd more");
    }

    #[test]
    fn test_planned_commit_from_hunk_ids() {
        let desc = CommitDescription::short_only("Test commit");
        let hunk_ids = vec![HunkId(0), HunkId(1), HunkId(2)];
        let commit = PlannedCommit::from_hunk_ids(PlannedCommitId(0), desc, hunk_ids);

        assert_eq!(commit.id.0, 0);
        assert_eq!(commit.description.short, "Test commit");
        assert_eq!(commit.changes.len(), 3);
        assert!(commit.depends_on.is_empty());

        // Verify they're all ExistingHunk variants
        for (i, change) in commit.changes.iter().enumerate() {
            match change {
                PlannedChange::ExistingHunk(id) => assert_eq!(id.0, i),
                PlannedChange::NewHunk(_) => panic!("Expected ExistingHunk"),
            }
        }
    }

    #[test]
    fn test_planned_commit_with_new_hunk() {
        let desc = CommitDescription::short_only("Test commit");
        let new_hunk = make_test_hunk();
        let changes = vec![
            PlannedChange::ExistingHunk(HunkId(5)),
            PlannedChange::NewHunk(new_hunk),
        ];
        let commit = PlannedCommit::new(PlannedCommitId(1), desc, changes);

        assert_eq!(commit.id.0, 1);
        assert_eq!(commit.changes.len(), 2);
        assert!(matches!(&commit.changes[0], PlannedChange::ExistingHunk(id) if id.0 == 5));
        assert!(matches!(&commit.changes[1], PlannedChange::NewHunk(_)));
    }

    #[test]
    fn test_planned_commit_with_dependencies() {
        let desc = CommitDescription::short_only("Dependent commit");
        let changes = vec![PlannedChange::ExistingHunk(HunkId(0))];
        let deps = vec![PlannedCommitId(0), PlannedCommitId(1)];
        let commit = PlannedCommit::with_dependencies(PlannedCommitId(2), desc, changes, deps);

        assert_eq!(commit.id.0, 2);
        assert_eq!(commit.depends_on.len(), 2);
        assert_eq!(commit.depends_on[0].0, 0);
        assert_eq!(commit.depends_on[1].0, 1);
    }

    #[test]
    fn test_hunk_id_equality() {
        let id1 = HunkId(5);
        let id2 = HunkId(5);
        let id3 = HunkId(6);

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_diff_line_variants() {
        let context = DiffLine::Context("unchanged".to_string());
        let added = DiffLine::Added("new line".to_string());
        let removed = DiffLine::Removed("old line".to_string());

        // Test equality
        assert_eq!(context, DiffLine::Context("unchanged".to_string()));
        assert_eq!(added, DiffLine::Added("new line".to_string()));
        assert_eq!(removed, DiffLine::Removed("old line".to_string()));

        // Test inequality
        assert_ne!(context, added);
        assert_ne!(added, removed);
    }
}
