//! File-based I/O for large LLM prompts
//!
//! This module provides temporary file management for the file-based LLM I/O
//! optimization. When enabled via the `FileBasedLlmIo` feature flag, hunks are
//! written to a temporary file and the LLM is instructed to read from that file,
//! reducing token usage for large diffs.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::llm::LlmError;

const REABSORB_TMP_DIR: &str = ".git/reabsorb/tmp";

/// Manages temporary input file for a single LLM invocation.
///
/// The input file is automatically cleaned up when the session is dropped.
pub struct LlmFileSession {
    /// Path to the input file containing hunks
    pub input_path: PathBuf,
    /// Whether to clean up files on drop
    cleanup_on_drop: bool,
}

impl LlmFileSession {
    /// Create a new session with a unique input file path.
    pub fn new() -> Result<Self, LlmError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_millis();

        let pid = std::process::id();

        let dir = PathBuf::from(REABSORB_TMP_DIR);
        fs::create_dir_all(&dir)?;

        Ok(Self {
            input_path: dir.join(format!("hunks-{}-{}.txt", timestamp, pid)),
            cleanup_on_drop: true,
        })
    }

    /// Write hunk content to the input file.
    pub fn write_input(&self, content: &str) -> Result<(), LlmError> {
        fs::write(&self.input_path, content)?;
        Ok(())
    }

    /// Disable automatic cleanup (useful for debugging).
    #[allow(dead_code)]
    pub fn keep_files(mut self) -> Self {
        self.cleanup_on_drop = false;
        self
    }
}

impl Drop for LlmFileSession {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            let _ = fs::remove_file(&self.input_path);
        }
    }
}

/// Extract a file path from LLM stdout and read its contents.
///
/// The LLM is expected to output the absolute path to a JSON file it created.
/// We find the path, read the file, and optionally copy it to our tmp dir.
pub fn read_response_from_path(stdout: &str) -> Result<String, LlmError> {
    // Look for lines that look like file paths
    let path = extract_file_path(stdout).ok_or_else(|| {
        LlmError::InvalidResponse(format!(
            "Could not find file path in LLM output: {}",
            stdout.chars().take(200).collect::<String>()
        ))
    })?;

    if !path.exists() {
        return Err(LlmError::InvalidResponse(format!(
            "LLM output file does not exist: {}",
            path.display()
        )));
    }

    let content = fs::read_to_string(&path)?;
    if content.trim().is_empty() {
        return Err(LlmError::InvalidResponse(
            "LLM output file exists but is empty".to_string(),
        ));
    }

    Ok(content)
}

/// Extract a file path from LLM output text.
///
/// Looks for absolute paths (starting with /) that end in .json
fn extract_file_path(text: &str) -> Option<PathBuf> {
    for line in text.lines() {
        let trimmed = line.trim();
        // Look for absolute paths ending in .json
        if trimmed.starts_with('/') && trimmed.ends_with(".json") {
            return Some(PathBuf::from(trimmed));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test session using a system temp directory
    fn test_session() -> LlmFileSession {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_nanos();

        let pid = std::process::id();
        let tid = std::thread::current().id();

        let dir = std::env::temp_dir().join("git-reabsorb-test");
        fs::create_dir_all(&dir).unwrap();

        LlmFileSession {
            input_path: dir.join(format!("hunks-{}-{}-{:?}.txt", timestamp, pid, tid)),
            cleanup_on_drop: true,
        }
    }

    #[test]
    fn test_session_creates_unique_paths() {
        let session1 = test_session();
        let session2 = test_session();

        assert_ne!(session1.input_path, session2.input_path);
    }

    #[test]
    fn test_write_input() {
        let session = test_session();
        let content = "test hunk content";

        session.write_input(content).unwrap();

        let read_content = fs::read_to_string(&session.input_path).unwrap();
        assert_eq!(read_content, content);
    }

    #[test]
    fn test_cleanup_on_drop() {
        let input_path;

        {
            let session = test_session();
            input_path = session.input_path.clone();

            session.write_input("test").unwrap();

            assert!(input_path.exists());
        }

        // File should be cleaned up after session drops
        assert!(!input_path.exists());
    }

    #[test]
    fn test_keep_files_prevents_cleanup() {
        let input_path;

        {
            let mut session = test_session();
            session.cleanup_on_drop = false;
            input_path = session.input_path.clone();

            session.write_input("test").unwrap();
        }

        // File should still exist
        assert!(input_path.exists());

        // Manual cleanup
        let _ = fs::remove_file(&input_path);
    }

    #[test]
    fn test_extract_file_path() {
        // Simple path on its own line
        let stdout = "/tmp/response.json\n";
        assert_eq!(
            extract_file_path(stdout),
            Some(PathBuf::from("/tmp/response.json"))
        );

        // Path with surrounding text
        let stdout = "Some output\n/path/to/file.json\nMore output";
        assert_eq!(
            extract_file_path(stdout),
            Some(PathBuf::from("/path/to/file.json"))
        );

        // No valid path
        let stdout = "Just some text\nno paths here";
        assert_eq!(extract_file_path(stdout), None);

        // Path without .json extension
        let stdout = "/path/to/file.txt\n";
        assert_eq!(extract_file_path(stdout), None);
    }

    #[test]
    fn test_read_response_from_path() {
        let dir = std::env::temp_dir().join("git-reabsorb-test");
        fs::create_dir_all(&dir).unwrap();

        let test_file = dir.join("test-response.json");
        let content = r#"{"commits": []}"#;
        fs::write(&test_file, content).unwrap();

        let stdout = format!("{}\n", test_file.display());
        let result = read_response_from_path(&stdout).unwrap();
        assert_eq!(result, content);

        // Cleanup
        let _ = fs::remove_file(&test_file);
    }

    #[test]
    fn test_read_response_from_path_missing_file() {
        let stdout = "/nonexistent/path.json\n";
        let result = read_response_from_path(stdout);
        assert!(result.is_err());
    }
}
