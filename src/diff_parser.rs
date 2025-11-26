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

/// Parse a unified diff output into hunks
pub fn parse_diff(
    diff_output: &str,
    source_commit_sha: &str,
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
                hunks.push(builder.build(source_commit_sha));
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
                hunks.push(builder.build(source_commit_sha));
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
                // Skip this marker
            } else if line.is_empty() {
                // Empty context line
                builder.lines.push(DiffLine::Context(String::new()));
            }
        }
    }

    // Finish final hunk
    if let Some(builder) = current_hunk.take() {
        hunks.push(builder.build(source_commit_sha));
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
}

impl HunkBuilder {
    fn build(self, source_commit_sha: &str) -> Hunk {
        Hunk {
            id: self.id,
            file_path: self.file_path,
            old_start: self.old_start,
            old_count: self.old_count,
            new_start: self.new_start,
            new_count: self.new_count,
            lines: self.lines,
            source_commit_sha: source_commit_sha.to_string(),
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

        let hunks = parse_diff(diff, "abc123", 0).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].file_path, PathBuf::from("src/main.rs"));
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 3);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 4);
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
}
