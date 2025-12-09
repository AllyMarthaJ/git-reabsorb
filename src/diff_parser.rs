use std::collections::HashMap;
use std::path::PathBuf;

use crate::models::{BinaryChangeType, BinaryFile, DiffLine, Hunk, HunkId, ModeChange};
use crate::patch::FileMode;

/// Errors that can occur during diff parsing
#[derive(Debug, thiserror::Error)]
pub enum DiffParseError {
    #[error("Invalid hunk header: {0}")]
    InvalidHunkHeader(String),
    #[error("Unexpected diff format: {0}")]
    UnexpectedFormat(String),
}

/// Result of parsing a diff - contains text hunks, binary files, and mode changes.
#[derive(Debug, Default)]
pub struct ParsedDiff {
    pub hunks: Vec<Hunk>,
    pub binary_files: Vec<BinaryFile>,
    /// Mode-only changes (files with mode change but no content change).
    /// For files with both content and mode changes, use `file_modes`.
    pub mode_changes: Vec<ModeChange>,
    /// File modes for all files that have non-default modes.
    /// This includes new files with specific modes, deleted files, and mode changes.
    pub file_modes: HashMap<PathBuf, FileMode>,
}

/// Parse a unified diff output into hunks.
///
/// `likely_source_commits` is a list of commit SHAs that likely contributed
/// to these hunks. For single-commit diffs (like `git show`), this is just
/// that commit. For working tree diffs, this can be determined by analyzing
/// which commits touched each file.
///
/// Note: This function ignores binary files. Use `parse_diff_full` if you need
/// to track binary file changes as well.
pub fn parse_diff(
    diff_output: &str,
    likely_source_commits: &[String],
    hunk_id_start: usize,
) -> Result<Vec<Hunk>, DiffParseError> {
    parse_diff_full(diff_output, likely_source_commits, hunk_id_start).map(|r| r.hunks)
}

/// Parse a unified diff output into hunks and binary file changes.
///
/// This is the full version that tracks both text hunks and binary file changes.
/// Use `parse_diff` if you only need text hunks.
pub fn parse_diff_full(
    diff_output: &str,
    likely_source_commits: &[String],
    hunk_id_start: usize,
) -> Result<ParsedDiff, DiffParseError> {
    let mut result = ParsedDiff::default();
    let mut current_file: Option<PathBuf> = None;
    let mut current_hunk: Option<HunkBuilder> = None;
    let mut hunk_id = hunk_id_start;
    // Track whether the current file is new or deleted (for binary files)
    let mut current_file_is_new = false;
    let mut current_file_is_deleted = false;
    // Track mode changes
    let mut current_old_mode: Option<String> = None;
    let mut current_new_mode: Option<String> = None;
    let mut current_file_has_hunks = false;
    let mut current_file_is_binary = false;

    // Helper to finalize file mode information for the previous file.
    // This stores:
    // 1. file_modes: Mode info for ALL files with non-default modes (new files, deleted files, mode changes)
    // 2. mode_changes: Mode-only changes (files with mode change but no content hunks)
    let finalize_file_mode = |result: &mut ParsedDiff,
                              file: &Option<PathBuf>,
                              old_mode: &Option<String>,
                              new_mode: &Option<String>,
                              is_new: bool,
                              is_deleted: bool,
                              has_hunks: bool,
                              is_binary: bool| {
        let Some(file_path) = file else { return };

        // Store file mode in file_modes map for patch generation
        if is_new {
            if let Some(mode) = new_mode {
                result
                    .file_modes
                    .insert(file_path.clone(), FileMode::New(mode.clone()));
            }
        } else if is_deleted {
            if let Some(mode) = old_mode {
                result
                    .file_modes
                    .insert(file_path.clone(), FileMode::Deleted(mode.clone()));
            }
        } else if let (Some(old), Some(new)) = (old_mode, new_mode) {
            // Mode change on existing file
            result.file_modes.insert(
                file_path.clone(),
                FileMode::Changed {
                    old: old.clone(),
                    new: new.clone(),
                },
            );

            // Also add to mode_changes for mode-only files (no content hunks)
            // Binary files have their mode handled when we `git add` them
            if !has_hunks && !is_binary {
                result.mode_changes.push(ModeChange {
                    file_path: file_path.clone(),
                    old_mode: old.clone(),
                    new_mode: new.clone(),
                    likely_source_commits: likely_source_commits.to_vec(),
                });
            }
        }
    };

    for line in diff_output.lines() {
        // New file diff header
        if line.starts_with("diff --git ") {
            // Finish any in-progress hunk
            if let Some(builder) = current_hunk.take() {
                result.hunks.push(builder.build(likely_source_commits));
            }

            // Finalize file mode for the previous file
            finalize_file_mode(
                &mut result,
                &current_file,
                &current_old_mode,
                &current_new_mode,
                current_file_is_new,
                current_file_is_deleted,
                current_file_has_hunks,
                current_file_is_binary,
            );

            // Parse file path from "diff --git a/path b/path"
            current_file = parse_diff_header(line);
            current_file_is_new = false;
            current_file_is_deleted = false;
            current_old_mode = None;
            current_new_mode = None;
            current_file_has_hunks = false;
            current_file_is_binary = false;
            continue;
        }

        // Track if this is a new file (and capture mode if present)
        // Format: "new file mode 100755" or just "new file"
        if let Some(rest) = line.strip_prefix("new file mode ") {
            current_file_is_new = true;
            current_new_mode = Some(rest.to_string());
            continue;
        }
        if line.starts_with("new file") {
            current_file_is_new = true;
            continue;
        }

        // Track if this is a deleted file (and capture mode if present)
        // Format: "deleted file mode 100644" or just "deleted file"
        if let Some(rest) = line.strip_prefix("deleted file mode ") {
            current_file_is_deleted = true;
            current_old_mode = Some(rest.to_string());
            continue;
        }
        if line.starts_with("deleted file") {
            current_file_is_deleted = true;
            continue;
        }

        // Track old mode (for mode-only changes on existing files)
        if let Some(mode) = line.strip_prefix("old mode ") {
            current_old_mode = Some(mode.to_string());
            continue;
        }

        // Track new mode (for mode-only changes on existing files)
        if let Some(mode) = line.strip_prefix("new mode ") {
            current_new_mode = Some(mode.to_string());
            continue;
        }

        // Handle --- line for deleted files (to get the path)
        if let Some(path) = line.strip_prefix("--- a/") {
            if current_file_is_deleted {
                current_file = Some(PathBuf::from(path));
            }
            continue;
        }

        // Skip other --- lines
        if line.starts_with("--- ") {
            continue;
        }

        // Handle file path from +++ line (more reliable for renames/new files)
        if line.starts_with("+++ ") {
            if let Some(path) = line.strip_prefix("+++ b/") {
                current_file = Some(PathBuf::from(path));
            }
            // +++ /dev/null means deleted, keep the path from ---
            continue;
        }

        // Handle binary files
        if line.starts_with("Binary files") {
            current_file_is_binary = true;
            if let Some(ref file_path) = current_file {
                let change_type = if current_file_is_new {
                    BinaryChangeType::Added
                } else if current_file_is_deleted {
                    BinaryChangeType::Deleted
                } else {
                    BinaryChangeType::Modified
                };

                result.binary_files.push(BinaryFile {
                    file_path: file_path.clone(),
                    change_type,
                    likely_source_commits: likely_source_commits.to_vec(),
                });
            }
            continue;
        }

        // Skip other metadata lines
        if line.starts_with("index ")
            || line.starts_with("similarity index")
            || line.starts_with("rename from")
            || line.starts_with("rename to")
        {
            continue;
        }

        // Hunk header
        if line.starts_with("@@ ") {
            current_file_has_hunks = true;
            // Finish any in-progress hunk
            if let Some(builder) = current_hunk.take() {
                result.hunks.push(builder.build(likely_source_commits));
            }

            // Parse hunk header
            let (old_start, old_count, new_start, new_count) = parse_hunk_header(line)?;

            current_hunk = Some(HunkBuilder {
                id: HunkId(hunk_id),
                file_path: current_file.clone().unwrap_or_default(),
                old_start,
                old_count,
                new_start,
                new_count,
                lines: Vec::new(),
                old_missing_newline_at_eof: false,
                new_missing_newline_at_eof: false,
            });
            hunk_id += 1;
            continue;
        }

        // Diff content lines
        if let Some(ref mut builder) = current_hunk {
            if let Some(content) = line.strip_prefix('+') {
                builder.lines.push(DiffLine::Added(content.to_string()));
            } else if let Some(content) = line.strip_prefix('-') {
                builder.lines.push(DiffLine::Removed(content.to_string()));
            } else if let Some(content) = line.strip_prefix(' ') {
                builder.lines.push(DiffLine::Context(content.to_string()));
            } else if line == "\\ No newline at end of file" {
                // Determine which side is missing the newline based on the last line type
                if let Some(last_line) = builder.lines.last() {
                    match last_line {
                        DiffLine::Removed(_) => builder.old_missing_newline_at_eof = true,
                        DiffLine::Added(_) => builder.new_missing_newline_at_eof = true,
                        DiffLine::Context(_) => {
                            // Context lines exist in both old and new
                            builder.old_missing_newline_at_eof = true;
                            builder.new_missing_newline_at_eof = true;
                        }
                    }
                }
            } else if line.is_empty() {
                // Empty context line
                builder.lines.push(DiffLine::Context(String::new()));
            }
        }
    }

    // Finish final hunk
    if let Some(builder) = current_hunk.take() {
        result.hunks.push(builder.build(likely_source_commits));
    }

    // Finalize file mode for the last file
    finalize_file_mode(
        &mut result,
        &current_file,
        &current_old_mode,
        &current_new_mode,
        current_file_is_new,
        current_file_is_deleted,
        current_file_has_hunks,
        current_file_is_binary,
    );

    Ok(result)
}

/// Parse the diff --git header to extract file path
fn parse_diff_header(line: &str) -> Option<PathBuf> {
    // Format: "diff --git a/path/to/file b/path/to/file"
    let rest = line.strip_prefix("diff --git ")?;
    let parts: Vec<&str> = rest.splitn(2, " b/").collect();
    if parts.len() == 2 {
        Some(PathBuf::from(parts[1]))
    } else {
        None
    }
}

/// Parse a hunk header like "@@ -1,5 +1,7 @@" or "@@ -1 +1,2 @@"
fn parse_hunk_header(line: &str) -> Result<(u32, u32, u32, u32), DiffParseError> {
    // Strip @@ prefix and suffix
    let content = line
        .strip_prefix("@@ ")
        .and_then(|s| s.split(" @@").next())
        .ok_or_else(|| DiffParseError::InvalidHunkHeader(line.to_string()))?;

    // Split into old and new parts: "-1,5 +1,7"
    let parts: Vec<&str> = content.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(DiffParseError::InvalidHunkHeader(line.to_string()));
    }

    let (old_start, old_count) = parse_range(parts[0].strip_prefix('-').unwrap_or(parts[0]))?;
    let (new_start, new_count) = parse_range(parts[1].strip_prefix('+').unwrap_or(parts[1]))?;

    Ok((old_start, old_count, new_start, new_count))
}

/// Parse a range like "1,5" or "1" into (start, count)
fn parse_range(s: &str) -> Result<(u32, u32), DiffParseError> {
    if let Some((start, count)) = s.split_once(',') {
        let start: u32 = start
            .parse()
            .map_err(|_| DiffParseError::InvalidHunkHeader(s.to_string()))?;
        let count: u32 = count
            .parse()
            .map_err(|_| DiffParseError::InvalidHunkHeader(s.to_string()))?;
        Ok((start, count))
    } else {
        let start: u32 = s
            .parse()
            .map_err(|_| DiffParseError::InvalidHunkHeader(s.to_string()))?;
        Ok((start, 1))
    }
}

/// Builder for constructing a Hunk
struct HunkBuilder {
    id: HunkId,
    file_path: PathBuf,
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
    lines: Vec<DiffLine>,
    old_missing_newline_at_eof: bool,
    new_missing_newline_at_eof: bool,
}

impl HunkBuilder {
    fn build(self, likely_source_commits: &[String]) -> Hunk {
        Hunk {
            id: self.id,
            file_path: self.file_path,
            old_start: self.old_start,
            old_count: self.old_count,
            new_start: self.new_start,
            new_count: self.new_count,
            lines: self.lines,
            likely_source_commits: likely_source_commits.to_vec(),
            old_missing_newline_at_eof: self.old_missing_newline_at_eof,
            new_missing_newline_at_eof: self.new_missing_newline_at_eof,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_diff() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
     println!("World");
 }
"#;

        let hunks = parse_diff(diff, &["abc123".to_string()], 0).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].file_path, PathBuf::from("src/main.rs"));
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 3);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 4);
        assert_eq!(hunks[0].likely_source_commits, vec!["abc123".to_string()]);
    }

    #[test]
    fn test_parse_hunk_header() {
        assert_eq!(parse_hunk_header("@@ -1,5 +1,7 @@").unwrap(), (1, 5, 1, 7));
        assert_eq!(parse_hunk_header("@@ -1 +1,2 @@").unwrap(), (1, 1, 1, 2));
        assert_eq!(
            parse_hunk_header("@@ -10,20 +15,25 @@ fn foo()").unwrap(),
            (10, 20, 15, 25)
        );
    }

    #[test]
    fn test_parse_diff_multiple_source_commits() {
        let diff = r#"diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,2 +1,3 @@
 line1
+line2
 line3
"#;

        let source_commits = vec!["commit1".to_string(), "commit2".to_string()];
        let hunks = parse_diff(diff, &source_commits, 0).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].likely_source_commits, source_commits);
    }

    #[test]
    fn test_parse_diff_multiple_hunks_same_file() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("start");
 }

@@ -10,3 +11,4 @@
 fn helper() {
+    println!("helper");
 }
"#;

        let hunks = parse_diff(diff, &[], 0).unwrap();
        assert_eq!(hunks.len(), 2);
        // Both hunks should be for the same file
        assert_eq!(hunks[0].file_path, hunks[1].file_path);
        // First hunk starts at line 1
        assert_eq!(hunks[0].old_start, 1);
        // Second hunk starts at line 10
        assert_eq!(hunks[1].old_start, 10);
    }

    #[test]
    fn test_parse_diff_multiple_files() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,2 +1,3 @@
 fn main() {
+    lib::greet();
 }
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,4 @@
 pub fn greet() {
+    println!("Hello");
 }
"#;

        let hunks = parse_diff(diff, &[], 0).unwrap();
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].file_path, PathBuf::from("src/main.rs"));
        assert_eq!(hunks[1].file_path, PathBuf::from("src/lib.rs"));
    }

    #[test]
    fn test_parse_diff_new_file() {
        let diff = r#"diff --git a/src/new.rs b/src/new.rs
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,3 @@
+fn new_function() {
+    println!("I'm new!");
+}
"#;

        let hunks = parse_diff(diff, &[], 0).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].file_path, PathBuf::from("src/new.rs"));
        // New file: old_start and old_count should be 0
        assert_eq!(hunks[0].old_start, 0);
        assert_eq!(hunks[0].old_count, 0);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 3);
    }

    #[test]
    fn test_parse_diff_deleted_file() {
        let diff = r#"diff --git a/src/old.rs b/src/old.rs
deleted file mode 100644
index 1234567..0000000
--- a/src/old.rs
+++ /dev/null
@@ -1,3 +0,0 @@
-fn old_function() {
-    println!("I'm being deleted!");
-}
"#;

        let hunks = parse_diff(diff, &[], 0).unwrap();
        assert_eq!(hunks.len(), 1);
        // File path should come from the --- line or diff header
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 3);
        assert_eq!(hunks[0].new_start, 0);
        assert_eq!(hunks[0].new_count, 0);
    }

    #[test]
    fn test_parse_diff_with_context_function_header() {
        // Git often includes the function name after the @@ header
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -5,6 +5,7 @@ fn some_function() {
     let x = 1;
     let y = 2;
+    let z = 3;
     println!("{}", x + y);
 }
"#;

        let hunks = parse_diff(diff, &[], 0).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 5);
        assert_eq!(hunks[0].old_count, 6);
        assert_eq!(hunks[0].new_start, 5);
        assert_eq!(hunks[0].new_count, 7);
    }

    #[test]
    fn test_parse_diff_empty_source_commits() {
        let diff = r#"diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1 +1,2 @@
 line1
+line2
"#;

        let hunks = parse_diff(diff, &[], 0).unwrap();
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].likely_source_commits.is_empty());
    }

    #[test]
    fn test_hunk_id_starts_from_provided_value() {
        let diff = r#"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1,2 @@
 line
+new
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1,2 @@
 line
+new
"#;

        // Start from hunk_id 100
        let hunks = parse_diff(diff, &[], 100).unwrap();
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].id.0, 100);
        assert_eq!(hunks[1].id.0, 101);
    }

    #[test]
    fn test_parse_binary_file_new() {
        let diff = r#"diff --git a/image.png b/image.png
new file mode 100644
index 0000000..abcdefg
Binary files /dev/null and b/image.png differ
"#;

        let result = parse_diff_full(diff, &["commit1".to_string()], 0).unwrap();
        assert_eq!(result.hunks.len(), 0);
        assert_eq!(result.binary_files.len(), 1);
        assert_eq!(result.binary_files[0].file_path, PathBuf::from("image.png"));
        assert_eq!(result.binary_files[0].change_type, BinaryChangeType::Added);
        assert_eq!(
            result.binary_files[0].likely_source_commits,
            vec!["commit1".to_string()]
        );
    }

    #[test]
    fn test_parse_binary_file_modified() {
        let diff = r#"diff --git a/image.png b/image.png
index 1234567..abcdefg 100644
Binary files a/image.png and b/image.png differ
"#;

        let result = parse_diff_full(diff, &[], 0).unwrap();
        assert_eq!(result.hunks.len(), 0);
        assert_eq!(result.binary_files.len(), 1);
        assert_eq!(result.binary_files[0].file_path, PathBuf::from("image.png"));
        assert_eq!(
            result.binary_files[0].change_type,
            BinaryChangeType::Modified
        );
    }

    #[test]
    fn test_parse_binary_file_deleted() {
        let diff = r#"diff --git a/image.png b/image.png
deleted file mode 100644
index abcdefg..0000000
Binary files a/image.png and /dev/null differ
"#;

        let result = parse_diff_full(diff, &[], 0).unwrap();
        assert_eq!(result.hunks.len(), 0);
        assert_eq!(result.binary_files.len(), 1);
        assert_eq!(result.binary_files[0].file_path, PathBuf::from("image.png"));
        assert_eq!(
            result.binary_files[0].change_type,
            BinaryChangeType::Deleted
        );
    }

    #[test]
    fn test_parse_mixed_text_and_binary() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,2 +1,3 @@
 fn main() {
+    println!("hello");
 }
diff --git a/image.png b/image.png
new file mode 100644
index 0000000..abcdefg
Binary files /dev/null and b/image.png differ
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1,2 @@
 pub fn foo() {}
+pub fn bar() {}
"#;

        let result = parse_diff_full(diff, &[], 0).unwrap();
        assert_eq!(result.hunks.len(), 2);
        assert_eq!(result.binary_files.len(), 1);
        assert_eq!(result.hunks[0].file_path, PathBuf::from("src/main.rs"));
        assert_eq!(result.hunks[1].file_path, PathBuf::from("src/lib.rs"));
        assert_eq!(result.binary_files[0].file_path, PathBuf::from("image.png"));
    }

    #[test]
    fn test_parse_mode_only_change() {
        let diff = r#"diff --git a/script.sh b/script.sh
old mode 100644
new mode 100755
"#;

        let result = parse_diff_full(diff, &["commit1".to_string()], 0).unwrap();
        assert_eq!(result.hunks.len(), 0);
        assert_eq!(result.binary_files.len(), 0);
        assert_eq!(result.mode_changes.len(), 1);
        assert_eq!(result.mode_changes[0].file_path, PathBuf::from("script.sh"));
        assert_eq!(result.mode_changes[0].old_mode, "100644");
        assert_eq!(result.mode_changes[0].new_mode, "100755");
        assert_eq!(
            result.mode_changes[0].likely_source_commits,
            vec!["commit1".to_string()]
        );
    }

    #[test]
    fn test_parse_mode_change_with_content_change() {
        // When a file has both mode change and content change,
        // the mode is stored in file_modes for patch generation.
        // mode_changes only contains mode-only changes (no content).
        let diff = r#"diff --git a/script.sh b/script.sh
old mode 100644
new mode 100755
index 1234567..abcdefg
--- a/script.sh
+++ b/script.sh
@@ -1 +1,2 @@
 echo "hello"
+echo "world"
"#;

        let result = parse_diff_full(diff, &[], 0).unwrap();
        assert_eq!(result.hunks.len(), 1);
        assert_eq!(result.binary_files.len(), 0);
        // mode_changes is empty because this file has content hunks
        assert_eq!(result.mode_changes.len(), 0);
        // Mode is stored in file_modes for patch generation
        assert_eq!(result.file_modes.len(), 1);
        let mode = result.file_modes.get(&PathBuf::from("script.sh")).unwrap();
        match mode {
            FileMode::Changed { old, new } => {
                assert_eq!(old, "100644");
                assert_eq!(new, "100755");
            }
            _ => panic!("Expected FileMode::Changed"),
        }
    }

    #[test]
    fn test_parse_multiple_mode_changes() {
        let diff = r#"diff --git a/script1.sh b/script1.sh
old mode 100644
new mode 100755
diff --git a/script2.sh b/script2.sh
old mode 100644
new mode 100755
"#;

        let result = parse_diff_full(diff, &[], 0).unwrap();
        assert_eq!(result.mode_changes.len(), 2);
        assert_eq!(
            result.mode_changes[0].file_path,
            PathBuf::from("script1.sh")
        );
        assert_eq!(
            result.mode_changes[1].file_path,
            PathBuf::from("script2.sh")
        );
    }
}
