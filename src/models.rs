use std::path::PathBuf;

/// Unique identifier for a hunk within a scramble operation
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HunkId(pub usize);

impl std::fmt::Display for HunkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "hunk#{}", self.0)
    }
}

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

/// A hunk represents a contiguous region of changes in a file.
///
/// These hunks are parsed from the unified diff between base and final state,
/// so all line numbers are relative to the same base. This means hunks can be
/// applied independently (as long as they don't overlap).
#[derive(Debug, Clone)]
pub struct Hunk {
    pub id: HunkId,
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
}

impl Hunk {
    /// Convert this hunk to unified diff format suitable for `git apply`
    #[must_use]
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

    /// Generate a full patch for this hunk (with file headers)
    #[must_use]
    pub fn to_full_patch(&self) -> String {
        let path_str = self.file_path.to_string_lossy();
        let mut patch = String::new();

        patch.push_str(&format!("--- a/{}\n", path_str));
        patch.push_str(&format!("+++ b/{}\n", path_str));
        patch.push_str(&self.to_patch());

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
    fn test_planned_commit_new() {
        let desc = CommitDescription::short_only("Test commit");
        let hunk_ids = vec![HunkId(0), HunkId(1), HunkId(2)];
        let commit = PlannedCommit::new(desc, hunk_ids);

        assert_eq!(commit.description.short, "Test commit");
        assert_eq!(commit.hunk_ids.len(), 3);
        assert_eq!(commit.hunk_ids[0].0, 0);
        assert_eq!(commit.hunk_ids[1].0, 1);
        assert_eq!(commit.hunk_ids[2].0, 2);
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
