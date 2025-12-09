//! Patch generation and application utilities.
//!
//! This module provides a centralized, correct implementation for generating
//! unified diff patches from hunks. It handles all edge cases:
//! - New files (--- /dev/null)
//! - Deleted files (+++ /dev/null)
//! - Modified files (normal diff)
//! - EOF newline handling
//! - Multiple hunks per file
//!
//! ## Patch Header Determination
//!
//! Patch headers are determined by combining two sources of information:
//!
//! 1. **Range context**: Whether the file is NEW in the commit range being
//!    reabsorbed. This is tracked via `new_files_to_commits` which records
//!    files that were created (not modified) during the range.
//!
//! 2. **Current index state**: Whether the file currently exists in the git
//!    index at the time of patch application.
//!
//! The `PatchContext` struct tracks both to ensure correct header generation.
//!
//! ## Key Rule
//!
//! If a file is NEW in the commit range (didn't exist at base), then ANY hunks
//! for that file must result in a "new file" patch (`--- /dev/null`), regardless
//! of the hunk's `old_count` metadata. The hunks are transformed via
//! `create_new_file_hunk` to extract only the content that should exist in the
//! final file.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::models::{DiffLine, Hunk};

/// File mode information from a diff.
#[derive(Debug, Clone)]
pub enum FileMode {
    /// New file with the given mode (e.g., "100755" for executable)
    New(String),
    /// Deleted file with the given mode
    Deleted(String),
    /// Mode change from old to new (e.g., 100644 -> 100755)
    Changed { old: String, new: String },
}

/// The type of change to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeType {
    /// File is being created (did not exist before)
    New,
    /// File is being modified (existed before and after)
    Modified,
    /// File is being deleted (will not exist after)
    Deleted,
}

impl FileChangeType {
    /// Determine the change type from a hunk's line counts.
    ///
    /// - `old_count == 0` and `new_count > 0`: New file
    /// - `old_count > 0` and `new_count == 0`: Deleted file
    /// - Otherwise: Modified file
    ///
    /// **Note**: This method only considers hunk metadata. For correct patch
    /// generation during execution, use `PatchContext::determine_change_type`
    /// which also considers range context and index state.
    #[must_use]
    pub fn from_hunk(hunk: &Hunk) -> Self {
        if hunk.old_count == 0 && hunk.new_count > 0 {
            FileChangeType::New
        } else if hunk.old_count > 0 && hunk.new_count == 0 {
            FileChangeType::Deleted
        } else {
            FileChangeType::Modified
        }
    }

    /// Determine the change type from multiple hunks for the same file.
    ///
    /// Uses the first hunk to determine the type, which should be consistent
    /// across all hunks for the same file in a well-formed diff.
    ///
    /// **Note**: This method only considers hunk metadata. For correct patch
    /// generation during execution, use `PatchContext::determine_change_type`.
    #[must_use]
    pub fn from_hunks(hunks: &[&Hunk]) -> Self {
        hunks
            .first()
            .map(|h| Self::from_hunk(h))
            .unwrap_or(FileChangeType::Modified)
    }
}

/// Context for generating patches during plan execution.
///
/// This struct tracks the information needed to determine correct patch headers:
/// - Which files are NEW in the commit range (from `new_files_to_commits`)
/// - Which files have been created by previous commits in the current execution
/// - Mode changes that need to be included in patch headers
///
/// ## Usage
///
/// Create a `PatchContext` at the start of plan execution, then use
/// `determine_change_type` for each file before generating patches.
#[derive(Debug, Clone)]
pub struct PatchContext {
    /// Files that are NEW in the commit range (didn't exist at base).
    /// Key is the file path as a string.
    new_files_in_range: HashSet<String>,
    /// File modes keyed by file path.
    file_modes: HashMap<String, FileMode>,
}

impl PatchContext {
    /// Create a new patch context from the new_files_to_commits map.
    ///
    /// The map keys are file paths of files that were created (not just modified)
    /// during the commit range being reabsorbed.
    pub fn new<S: AsRef<str>>(new_files: impl IntoIterator<Item = S>) -> Self {
        Self {
            new_files_in_range: new_files
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
            file_modes: HashMap::new(),
        }
    }

    /// Create a patch context with file modes from parsed diff.
    pub fn with_file_modes<S: AsRef<str>>(
        new_files: impl IntoIterator<Item = S>,
        file_modes: &HashMap<std::path::PathBuf, FileMode>,
    ) -> Self {
        Self {
            new_files_in_range: new_files
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
            file_modes: file_modes
                .iter()
                .map(|(k, v)| (k.to_string_lossy().to_string(), v.clone()))
                .collect(),
        }
    }

    /// Create an empty context (for cases where range info isn't available).
    pub fn empty() -> Self {
        Self {
            new_files_in_range: HashSet::new(),
            file_modes: HashMap::new(),
        }
    }

    /// Get the file mode for a file, if any.
    pub fn get_file_mode(&self, file_path: &Path) -> Option<&FileMode> {
        let path_str = file_path.to_string_lossy();
        self.file_modes.get(path_str.as_ref())
    }

    /// Check if a file is NEW in the commit range.
    ///
    /// Returns true if the file didn't exist at the base commit and was
    /// created during the range being reabsorbed.
    pub fn is_new_in_range(&self, file_path: &Path) -> bool {
        let path_str = file_path.to_string_lossy();
        self.new_files_in_range.contains(path_str.as_ref())
    }

    /// Determine the correct change type for a file.
    ///
    /// This is the **primary method** for determining patch headers during
    /// plan execution. It combines:
    /// 1. Range context (is file new in the commit range?)
    /// 2. Current index state (does file exist in index?)
    /// 3. Hunk metadata (are we deleting the file?)
    ///
    /// # Arguments
    /// * `file_path` - Path to the file
    /// * `file_in_index` - Whether the file currently exists in the git index
    /// * `hunks` - The hunks being applied to this file
    ///
    /// # Returns
    /// The `FileChangeType` to use for generating the patch headers.
    pub fn determine_change_type(
        &self,
        file_path: &Path,
        file_in_index: bool,
        hunks: &[&Hunk],
    ) -> FileChangeType {
        // Check if all hunks indicate deletion (new_count == 0)
        let is_deletion = hunks.iter().all(|h| h.new_count == 0);

        if is_deletion && file_in_index {
            // File exists and all hunks are deletions -> delete file
            return FileChangeType::Deleted;
        }

        if file_in_index {
            // File exists in index -> this is a modification
            return FileChangeType::Modified;
        }

        // File doesn't exist in index. Determine if it's a new file.
        //
        // Priority:
        // 1. If file is marked as new in range -> New
        // 2. If hunks indicate new file (old_count == 0) -> New
        // 3. Otherwise -> New (we can't modify a file that doesn't exist)

        // If file is NEW in the commit range, always treat as new file
        if self.is_new_in_range(file_path) {
            return FileChangeType::New;
        }

        // Check hunk metadata - if all hunks show new file, treat as new
        let hunk_indicates_new = hunks.iter().all(|h| h.old_count == 0);
        if hunk_indicates_new {
            return FileChangeType::New;
        }

        // File doesn't exist in index and has modification hunks.
        // This happens when:
        // - File was deleted earlier in the plan execution
        // - Or plan is reorganized such that modification hunks come before creation
        //
        // We must treat this as a new file and transform the hunks.
        FileChangeType::New
    }

    /// Generate a complete patch for a file with correct headers.
    ///
    /// This is the **primary method** for generating patches during plan execution.
    /// It:
    /// 1. Determines the correct change type using `determine_change_type`
    /// 2. Transforms hunks if needed (e.g., modification hunks -> new file hunks)
    /// 3. Generates the patch with correct headers (including mode changes)
    ///
    /// # Arguments
    /// * `file_path` - Path to the file
    /// * `hunks` - The hunks to include in the patch (must be sorted by old_start)
    /// * `file_in_index` - Whether the file currently exists in the git index
    ///
    /// # Returns
    /// A tuple of (patch_string, change_type_used)
    pub fn generate_patch(
        &self,
        file_path: &Path,
        hunks: &[&Hunk],
        file_in_index: bool,
    ) -> (String, FileChangeType) {
        let change_type = self.determine_change_type(file_path, file_in_index, hunks);
        let file_mode = self.get_file_mode(file_path);

        let patch = match change_type {
            FileChangeType::New => {
                // For new files, we need to transform hunks to extract only the
                // content that should exist in the final file.
                let new_file_hunk = PatchWriter::create_new_file_hunk(file_path, hunks);
                PatchWriter::write_patch_with_file_mode(
                    file_path,
                    &[&new_file_hunk],
                    FileChangeType::New,
                    file_mode,
                )
            }
            FileChangeType::Deleted => {
                // For deletions, transform hunks to show content being removed
                let delete_hunk = PatchWriter::create_delete_file_hunk(file_path, hunks);
                PatchWriter::write_patch_with_file_mode(
                    file_path,
                    &[&delete_hunk],
                    FileChangeType::Deleted,
                    file_mode,
                )
            }
            FileChangeType::Modified => {
                // For modifications, use hunks as-is with proper headers
                PatchWriter::write_patch_with_file_mode(
                    file_path,
                    hunks,
                    FileChangeType::Modified,
                    file_mode,
                )
            }
        };

        (patch, change_type)
    }
}

/// Generates unified diff patches from hunks.
///
/// This is the single source of truth for patch generation in the codebase.
/// All patch generation should go through this struct to ensure consistency.
pub struct PatchWriter;

impl PatchWriter {
    /// Generate a complete patch for a single hunk with file headers.
    ///
    /// The file change type is inferred from the hunk's line counts.
    #[must_use]
    pub fn write_single_hunk(hunk: &Hunk) -> String {
        let change_type = FileChangeType::from_hunk(hunk);
        Self::write_patch(&hunk.file_path, &[hunk], change_type)
    }

    /// Generate a complete patch for multiple hunks of the same file.
    ///
    /// Hunks should be sorted by `old_start` before calling this function.
    /// The file change type is inferred from the first hunk.
    #[must_use]
    pub fn write_multi_hunk(file_path: &Path, hunks: &[&Hunk]) -> String {
        let change_type = FileChangeType::from_hunks(hunks);
        Self::write_patch(file_path, hunks, change_type)
    }

    /// Generate a complete patch with explicit file change type.
    ///
    /// This is the core patch generation function. Use this when you need
    /// explicit control over the file change type (e.g., when creating a
    /// new file from hunks that weren't originally for a new file).
    #[must_use]
    pub fn write_patch<H: AsRef<Hunk>>(
        file_path: &Path,
        hunks: &[H],
        change_type: FileChangeType,
    ) -> String {
        Self::write_patch_with_file_mode(file_path, hunks, change_type, None)
    }

    /// Generate a complete patch with explicit file change type and optional file mode.
    ///
    /// This includes the extended header lines for file modes:
    /// - New files: `new file mode <mode>`
    /// - Deleted files: `deleted file mode <mode>`
    /// - Mode changes: `old mode <old>` + `new mode <new>`
    #[must_use]
    pub fn write_patch_with_file_mode<H: AsRef<Hunk>>(
        file_path: &Path,
        hunks: &[H],
        change_type: FileChangeType,
        file_mode: Option<&FileMode>,
    ) -> String {
        let mut patch = String::new();
        let path_str = file_path.to_string_lossy();

        // Git extended headers require the "diff --git" line first
        if file_mode.is_some() {
            patch.push_str(&format!("diff --git a/{} b/{}\n", path_str, path_str));
        }

        // Add mode headers based on file mode type (before --- and +++ lines)
        if let Some(mode) = file_mode {
            match mode {
                FileMode::New(m) => {
                    patch.push_str(&format!("new file mode {}\n", m));
                }
                FileMode::Deleted(m) => {
                    patch.push_str(&format!("deleted file mode {}\n", m));
                }
                FileMode::Changed { old, new } => {
                    patch.push_str(&format!("old mode {}\n", old));
                    patch.push_str(&format!("new mode {}\n", new));
                }
            }
        }

        // Generate file headers based on change type
        let (old_path, new_path) = match change_type {
            FileChangeType::New => ("/dev/null".to_string(), format!("b/{}", path_str)),
            FileChangeType::Modified => (format!("a/{}", path_str), format!("b/{}", path_str)),
            FileChangeType::Deleted => (format!("a/{}", path_str), "/dev/null".to_string()),
        };

        patch.push_str(&format!("--- {}\n", old_path));
        patch.push_str(&format!("+++ {}\n", new_path));

        // Write each hunk
        for hunk in hunks {
            patch.push_str(&Self::write_hunk_body(hunk.as_ref()));
        }

        patch
    }

    /// Generate the body of a hunk (header + content lines).
    ///
    /// This generates just the `@@ ... @@` header and the diff lines,
    /// without the file headers (`---`/`+++`).
    #[must_use]
    pub fn write_hunk_body(hunk: &Hunk) -> String {
        let mut output = String::new();

        // Hunk header
        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        ));

        // Find positions of last lines that touch EOF for each side
        let last_old_idx = hunk
            .lines
            .iter()
            .rposition(|line| matches!(line, DiffLine::Removed(_) | DiffLine::Context(_)));
        let last_new_idx = hunk
            .lines
            .iter()
            .rposition(|line| matches!(line, DiffLine::Added(_) | DiffLine::Context(_)));

        // Write diff lines with proper prefixes
        for (idx, line) in hunk.lines.iter().enumerate() {
            match line {
                DiffLine::Context(s) => {
                    output.push(' ');
                    output.push_str(s);
                    output.push('\n');
                    // Context lines affect both old and new
                    if Some(idx) == last_old_idx
                        && Some(idx) == last_new_idx
                        && (hunk.old_missing_newline_at_eof || hunk.new_missing_newline_at_eof)
                    {
                        output.push_str("\\ No newline at end of file\n");
                    }
                }
                DiffLine::Removed(s) => {
                    output.push('-');
                    output.push_str(s);
                    output.push('\n');
                    if Some(idx) == last_old_idx && hunk.old_missing_newline_at_eof {
                        output.push_str("\\ No newline at end of file\n");
                    }
                }
                DiffLine::Added(s) => {
                    output.push('+');
                    output.push_str(s);
                    output.push('\n');
                    if Some(idx) == last_new_idx && hunk.new_missing_newline_at_eof {
                        output.push_str("\\ No newline at end of file\n");
                    }
                }
            }
        }

        output
    }

    /// Create a "new file" hunk from multiple hunks.
    ///
    /// This combines multiple hunks into a single hunk that creates a new file.
    /// It correctly handles the transformation:
    /// - `Added` lines are included (they end up in the new file)
    /// - `Context` lines are included (they exist in the final state)
    /// - `Removed` lines are EXCLUDED (they represent deleted content)
    ///
    /// This is the correct way to create a new file from hunks that may have
    /// been parsed from modifications to an existing file.
    #[must_use]
    pub fn create_new_file_hunk(file_path: &Path, hunks: &[&Hunk]) -> Hunk {
        use crate::models::HunkId;

        let mut new_lines = Vec::new();

        for hunk in hunks {
            for line in &hunk.lines {
                match line {
                    DiffLine::Added(s) | DiffLine::Context(s) => {
                        // These lines exist in the final file
                        new_lines.push(DiffLine::Added(s.clone()));
                    }
                    DiffLine::Removed(_) => {
                        // Removed lines do NOT exist in the final file - skip them
                    }
                }
            }
        }

        let new_count = new_lines.len() as u32;

        Hunk {
            id: hunks.first().map(|h| h.id).unwrap_or(HunkId(0)),
            file_path: file_path.to_path_buf(),
            old_start: 0,
            old_count: 0,
            new_start: if new_count > 0 { 1 } else { 0 },
            new_count,
            lines: new_lines,
            likely_source_commits: hunks
                .iter()
                .flat_map(|h| h.likely_source_commits.iter().cloned())
                .collect(),
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: hunks
                .last()
                .map(|h| h.new_missing_newline_at_eof)
                .unwrap_or(false),
        }
    }

    /// Create a "delete file" hunk from multiple hunks.
    ///
    /// This combines multiple hunks into a single hunk that deletes a file.
    /// It correctly transforms all content to removed lines.
    #[must_use]
    pub fn create_delete_file_hunk(file_path: &Path, hunks: &[&Hunk]) -> Hunk {
        use crate::models::HunkId;

        let mut removed_lines = Vec::new();

        for hunk in hunks {
            for line in &hunk.lines {
                match line {
                    DiffLine::Removed(s) | DiffLine::Context(s) => {
                        // These lines exist in the old file
                        removed_lines.push(DiffLine::Removed(s.clone()));
                    }
                    DiffLine::Added(_) => {
                        // Added lines don't exist in the old file - skip them
                    }
                }
            }
        }

        let old_count = removed_lines.len() as u32;

        Hunk {
            id: hunks.first().map(|h| h.id).unwrap_or(HunkId(0)),
            file_path: file_path.to_path_buf(),
            old_start: if old_count > 0 { 1 } else { 0 },
            old_count,
            new_start: 0,
            new_count: 0,
            lines: removed_lines,
            likely_source_commits: hunks
                .iter()
                .flat_map(|h| h.likely_source_commits.iter().cloned())
                .collect(),
            old_missing_newline_at_eof: hunks
                .last()
                .map(|h| h.old_missing_newline_at_eof)
                .unwrap_or(false),
            new_missing_newline_at_eof: false,
        }
    }
}

impl AsRef<Hunk> for Hunk {
    fn as_ref(&self) -> &Hunk {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::HunkId;
    use std::path::PathBuf;

    fn make_simple_hunk() -> Hunk {
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

    fn make_new_file_hunk() -> Hunk {
        Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("src/new.rs"),
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: 3,
            lines: vec![
                DiffLine::Added("fn new() {".to_string()),
                DiffLine::Added("    // new file".to_string()),
                DiffLine::Added("}".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        }
    }

    fn make_deleted_file_hunk() -> Hunk {
        Hunk {
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
        }
    }

    #[test]
    fn test_file_change_type_from_hunk() {
        let new = make_new_file_hunk();
        assert_eq!(FileChangeType::from_hunk(&new), FileChangeType::New);

        let deleted = make_deleted_file_hunk();
        assert_eq!(FileChangeType::from_hunk(&deleted), FileChangeType::Deleted);

        let modified = make_simple_hunk();
        assert_eq!(
            FileChangeType::from_hunk(&modified),
            FileChangeType::Modified
        );
    }

    #[test]
    fn test_write_single_hunk_modified() {
        let hunk = make_simple_hunk();
        let patch = PatchWriter::write_single_hunk(&hunk);

        assert!(patch.contains("--- a/src/main.rs"));
        assert!(patch.contains("+++ b/src/main.rs"));
        assert!(patch.contains("@@ -1,3 +1,4 @@"));
        assert!(patch.contains(" fn main() {"));
        assert!(patch.contains("+    println!(\"Hello\");"));
    }

    #[test]
    fn test_write_single_hunk_new_file() {
        let hunk = make_new_file_hunk();
        let patch = PatchWriter::write_single_hunk(&hunk);

        assert!(patch.contains("--- /dev/null"));
        assert!(patch.contains("+++ b/src/new.rs"));
        assert!(patch.contains("@@ -0,0 +1,3 @@"));
        assert!(patch.contains("+fn new() {"));
    }

    #[test]
    fn test_write_single_hunk_deleted_file() {
        let hunk = make_deleted_file_hunk();
        let patch = PatchWriter::write_single_hunk(&hunk);

        assert!(patch.contains("--- a/src/old.rs"));
        assert!(patch.contains("+++ /dev/null"));
        assert!(patch.contains("@@ -1,3 +0,0 @@"));
        assert!(patch.contains("-fn old() {"));
    }

    #[test]
    fn test_write_patch_with_explicit_type() {
        let hunk = make_simple_hunk();

        // Force it to be treated as a new file
        let patch = PatchWriter::write_patch(&hunk.file_path, &[&hunk], FileChangeType::New);

        assert!(patch.contains("--- /dev/null"));
        assert!(patch.contains("+++ b/src/main.rs"));
    }

    #[test]
    fn test_create_new_file_hunk_from_modification() {
        // Simulate a hunk with removed content
        let modification_hunk = Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("src/file.rs"),
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 2,
            lines: vec![
                DiffLine::Context("line1".to_string()),
                DiffLine::Removed("old_line".to_string()),
                DiffLine::Added("new_line".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };

        let new_hunk =
            PatchWriter::create_new_file_hunk(&PathBuf::from("src/file.rs"), &[&modification_hunk]);

        // Should only have the lines that exist in the final file
        assert_eq!(new_hunk.old_count, 0);
        assert_eq!(new_hunk.new_count, 2); // line1 + new_line
        assert_eq!(new_hunk.lines.len(), 2);

        // Both should be Added lines
        assert!(matches!(&new_hunk.lines[0], DiffLine::Added(s) if s == "line1"));
        assert!(matches!(&new_hunk.lines[1], DiffLine::Added(s) if s == "new_line"));

        // The removed line should NOT be included
        let has_old_line = new_hunk
            .lines
            .iter()
            .any(|l| matches!(l, DiffLine::Added(s) | DiffLine::Context(s) if s == "old_line"));
        assert!(
            !has_old_line,
            "Removed lines should not be included in new file hunk"
        );
    }

    #[test]
    fn test_create_new_file_hunk_from_multiple_hunks() {
        let hunk1 = Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("src/file.rs"),
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: 2,
            lines: vec![
                DiffLine::Added("line1".to_string()),
                DiffLine::Added("line2".to_string()),
            ],
            likely_source_commits: vec!["commit1".to_string()],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };

        let hunk2 = Hunk {
            id: HunkId(1),
            file_path: PathBuf::from("src/file.rs"),
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 3,
            lines: vec![
                DiffLine::Context("line1".to_string()),
                DiffLine::Removed("line2".to_string()),
                DiffLine::Added("modified_line2".to_string()),
                DiffLine::Added("line3".to_string()),
            ],
            likely_source_commits: vec!["commit2".to_string()],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };

        let new_hunk =
            PatchWriter::create_new_file_hunk(&PathBuf::from("src/file.rs"), &[&hunk1, &hunk2]);

        // Should combine: line1, line2 from hunk1, then line1, modified_line2, line3 from hunk2
        // (excluding removed lines)
        assert_eq!(new_hunk.old_count, 0);
        assert_eq!(new_hunk.new_count, 5);

        // All lines should be Added
        for line in &new_hunk.lines {
            assert!(matches!(line, DiffLine::Added(_)));
        }

        // Source commits should be combined
        assert!(new_hunk
            .likely_source_commits
            .contains(&"commit1".to_string()));
        assert!(new_hunk
            .likely_source_commits
            .contains(&"commit2".to_string()));
    }

    #[test]
    fn test_create_delete_file_hunk() {
        let hunk = Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("src/file.rs"),
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 2,
            lines: vec![
                DiffLine::Context("line1".to_string()),
                DiffLine::Removed("line2".to_string()),
                DiffLine::Context("line3".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };

        let delete_hunk =
            PatchWriter::create_delete_file_hunk(&PathBuf::from("src/file.rs"), &[&hunk]);

        // Should have all lines that existed in the old file as Removed
        assert_eq!(delete_hunk.old_count, 3);
        assert_eq!(delete_hunk.new_count, 0);
        assert_eq!(delete_hunk.lines.len(), 3);

        for line in &delete_hunk.lines {
            assert!(matches!(line, DiffLine::Removed(_)));
        }
    }

    #[test]
    fn test_write_hunk_body() {
        let hunk = make_simple_hunk();
        let body = PatchWriter::write_hunk_body(&hunk);

        assert!(body.starts_with("@@ -1,3 +1,4 @@\n"));
        assert!(body.contains(" fn main() {\n"));
        assert!(body.contains("+    println!(\"Hello\");\n"));
    }

    #[test]
    fn test_eof_newline_handling() {
        let hunk = Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("file.txt"),
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            lines: vec![
                DiffLine::Removed("old".to_string()),
                DiffLine::Added("new".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: true,
            new_missing_newline_at_eof: true,
        };

        let body = PatchWriter::write_hunk_body(&hunk);

        // Should have two "No newline" markers - one after removed, one after added
        let marker_count = body.matches("\\ No newline at end of file").count();
        assert_eq!(marker_count, 2);
    }

    #[test]
    fn test_multi_hunk_patch() {
        let hunk1 = Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("src/main.rs"),
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 3,
            lines: vec![
                DiffLine::Context("line1".to_string()),
                DiffLine::Added("added1".to_string()),
                DiffLine::Context("line2".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };

        let hunk2 = Hunk {
            id: HunkId(1),
            file_path: PathBuf::from("src/main.rs"),
            old_start: 10,
            old_count: 2,
            new_start: 11,
            new_count: 3,
            lines: vec![
                DiffLine::Context("line10".to_string()),
                DiffLine::Added("added10".to_string()),
                DiffLine::Context("line11".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };

        let patch = PatchWriter::write_multi_hunk(&PathBuf::from("src/main.rs"), &[&hunk1, &hunk2]);

        // Should have one set of file headers
        assert_eq!(patch.matches("--- a/src/main.rs").count(), 1);
        assert_eq!(patch.matches("+++ b/src/main.rs").count(), 1);

        // Should have two hunk headers
        assert!(patch.contains("@@ -1,2 +1,3 @@"));
        assert!(patch.contains("@@ -10,2 +11,3 @@"));
    }

    // =========================================================================
    // PatchContext Tests
    // =========================================================================

    #[test]
    fn test_patch_context_empty() {
        let ctx = PatchContext::empty();
        assert!(!ctx.is_new_in_range(Path::new("any/file.rs")));
    }

    #[test]
    fn test_patch_context_with_new_files() {
        let ctx = PatchContext::new(["src/new.rs", "src/other.rs"]);
        assert!(ctx.is_new_in_range(Path::new("src/new.rs")));
        assert!(ctx.is_new_in_range(Path::new("src/other.rs")));
        assert!(!ctx.is_new_in_range(Path::new("src/existing.rs")));
    }

    #[test]
    fn test_determine_change_type_existing_file() {
        let ctx = PatchContext::empty();
        let hunk = make_simple_hunk();

        // File exists in index -> modification
        let change_type = ctx.determine_change_type(Path::new("src/main.rs"), true, &[&hunk]);
        assert_eq!(change_type, FileChangeType::Modified);
    }

    #[test]
    fn test_determine_change_type_new_file_by_hunk() {
        let ctx = PatchContext::empty();
        let hunk = make_new_file_hunk();

        // File not in index and hunk indicates new file -> new
        let change_type = ctx.determine_change_type(Path::new("src/new.rs"), false, &[&hunk]);
        assert_eq!(change_type, FileChangeType::New);
    }

    #[test]
    fn test_determine_change_type_new_file_by_range() {
        let ctx = PatchContext::new(["src/new.rs"]);
        let hunk = make_simple_hunk(); // This is a modification hunk

        // File not in index but marked as new in range -> new
        // Even though hunk looks like modification (old_count > 0)
        let change_type = ctx.determine_change_type(Path::new("src/new.rs"), false, &[&hunk]);
        assert_eq!(change_type, FileChangeType::New);
    }

    #[test]
    fn test_determine_change_type_modification_hunk_no_file_becomes_new() {
        let ctx = PatchContext::empty();
        let hunk = make_simple_hunk(); // Has old_count > 0

        // File not in index, modification hunk, not marked as new in range
        // Should still become New (we can't modify a non-existent file)
        let change_type = ctx.determine_change_type(Path::new("src/main.rs"), false, &[&hunk]);
        assert_eq!(change_type, FileChangeType::New);
    }

    #[test]
    fn test_determine_change_type_deletion() {
        let ctx = PatchContext::empty();
        let hunk = make_deleted_file_hunk();

        // File exists in index and all hunks have new_count == 0 -> deletion
        let change_type = ctx.determine_change_type(Path::new("src/old.rs"), true, &[&hunk]);
        assert_eq!(change_type, FileChangeType::Deleted);
    }

    #[test]
    fn test_generate_patch_transforms_modification_to_new_file() {
        let ctx = PatchContext::new(["src/new.rs"]);

        // Create a modification hunk (with context and removed lines)
        let hunk = Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("src/new.rs"),
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 2,
            lines: vec![
                DiffLine::Context("line1".to_string()),
                DiffLine::Removed("old_line2".to_string()),
                DiffLine::Added("new_line2".to_string()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        };

        // Generate patch for file that's new in range but not in index
        let (patch, change_type) = ctx.generate_patch(Path::new("src/new.rs"), &[&hunk], false);

        // Should be treated as new file
        assert_eq!(change_type, FileChangeType::New);
        assert!(patch.contains("--- /dev/null"));
        assert!(patch.contains("+++ b/src/new.rs"));

        // Patch should have transformed content (only Added lines)
        assert!(patch.contains("+line1"));
        assert!(patch.contains("+new_line2"));
        // Should NOT contain the removed line
        assert!(!patch.contains("old_line2"));
    }

    #[test]
    fn test_generate_patch_deletion() {
        let ctx = PatchContext::empty();
        let hunk = make_deleted_file_hunk();

        let (patch, change_type) = ctx.generate_patch(Path::new("src/old.rs"), &[&hunk], true);

        assert_eq!(change_type, FileChangeType::Deleted);
        assert!(patch.contains("--- a/src/old.rs"));
        assert!(patch.contains("+++ /dev/null"));
    }

    #[test]
    fn test_generate_patch_modification() {
        let ctx = PatchContext::empty();
        let hunk = make_simple_hunk();

        let (patch, change_type) = ctx.generate_patch(Path::new("src/main.rs"), &[&hunk], true);

        assert_eq!(change_type, FileChangeType::Modified);
        assert!(patch.contains("--- a/src/main.rs"));
        assert!(patch.contains("+++ b/src/main.rs"));
        // Line numbers should be preserved for modifications
        assert!(patch.contains("@@ -1,3 +1,4 @@"));
    }
}
