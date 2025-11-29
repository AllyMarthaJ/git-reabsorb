use std::path::PathBuf;

use crate::models::{DiffLine, Hunk, HunkId};

/// Errors that can occur during diff parsing
#[derive(Debug, thiserror::Error)]
pub enum DiffParseError {
    #[error("Invalid hunk header: {0}")]
    InvalidHunkHeader(String),
    #[error("Unexpected diff format: {0}")]
    UnexpectedFormat(String),
}

/// Parse a unified diff output into hunks.
///
/// `likely_source_commits` is a list of commit SHAs that likely contributed
/// to these hunks. For single-commit diffs (like `git show`), this is just
/// that commit. For working tree diffs, this can be determined by analyzing
/// which commits touched each file.
pub fn parse_diff(
    diff_output: &str,
    likely_source_commits: &[String],
    hunk_id_start: usize,
) -> Result<Vec<Hunk>, DiffParseError> {
    let mut hunks = Vec::new();
    let mut current_file: Option<PathBuf> = None;
    let mut current_hunk: Option<HunkBuilder> = None;
    let mut hunk_id = hunk_id_start;

    for line in diff_output.lines() {
        // New file diff header
        if line.starts_with("diff --git ") {
            // Finish any in-progress hunk
            if let Some(builder) = current_hunk.take() {
                hunks.push(builder.build(likely_source_commits));
            }

            // Parse file path from "diff --git a/path b/path"
            current_file = parse_diff_header(line);
            continue;
        }

        // Handle file path from +++ line (more reliable for renames/new files)
        if line.starts_with("+++ ") {
            if let Some(path) = line.strip_prefix("+++ b/") {
                current_file = Some(PathBuf::from(path));
            } else if line.starts_with("+++ /dev/null") {
                // File was deleted, keep the old path from ---
            }
            continue;
        }

        // Skip --- lines, index lines, etc.
        if line.starts_with("--- ")
            || line.starts_with("index ")
            || line.starts_with("new file")
            || line.starts_with("deleted file")
            || line.starts_with("old mode")
            || line.starts_with("new mode")
            || line.starts_with("similarity index")
            || line.starts_with("rename from")
            || line.starts_with("rename to")
            || line.starts_with("Binary files")
        {
            continue;
        }

        // Hunk header
        if line.starts_with("@@ ") {
            // Finish any in-progress hunk
            if let Some(builder) = current_hunk.take() {
                hunks.push(builder.build(likely_source_commits));
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
        hunks.push(builder.build(likely_source_commits));
    }

    Ok(hunks)
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
}
