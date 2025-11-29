//! Shared test utilities for creating test fixtures.
//!
//! This module provides helper functions for creating test data
//! used across multiple test modules.

use crate::models::{DiffLine, Hunk, HunkId, SourceCommit};
use std::path::PathBuf;

/// Create a minimal test hunk with default values
pub fn make_hunk(id: usize) -> Hunk {
    make_hunk_in_file(id, "test.rs")
}

/// Create a test hunk in a specific file
pub fn make_hunk_in_file(id: usize, file: &str) -> Hunk {
    make_hunk_with_source(id, file, vec![])
}

/// Create a test hunk with a specific source commit SHA
pub fn make_hunk_with_source(id: usize, file: &str, source_commits: Vec<String>) -> Hunk {
    make_hunk_full(
        id,
        file,
        vec![DiffLine::Added("test".to_string())],
        source_commits,
    )
}

/// Create a fully customized test hunk
pub fn make_hunk_full(
    id: usize,
    file: &str,
    lines: Vec<DiffLine>,
    source_commits: Vec<String>,
) -> Hunk {
    Hunk {
        id: HunkId(id),
        file_path: PathBuf::from(file),
        old_start: 1,
        old_count: 1,
        new_start: 1,
        new_count: 1,
        lines,
        likely_source_commits: source_commits,
        old_missing_newline_at_eof: false,
        new_missing_newline_at_eof: false,
    }
}

/// Create a test source commit
pub fn make_source_commit(sha: &str, message: &str) -> SourceCommit {
    SourceCommit::new(sha, message, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_hunk() {
        let hunk = make_hunk(5);
        assert_eq!(hunk.id, HunkId(5));
        assert_eq!(hunk.file_path, PathBuf::from("test.rs"));
    }

    #[test]
    fn test_make_hunk_in_file() {
        let hunk = make_hunk_in_file(3, "src/main.rs");
        assert_eq!(hunk.id, HunkId(3));
        assert_eq!(hunk.file_path, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn test_make_source_commit() {
        let commit = make_source_commit("abc123", "Test message");
        assert_eq!(commit.sha, "abc123");
        assert_eq!(commit.message.short, "Test message");
    }
}
