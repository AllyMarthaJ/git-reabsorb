use std::env;
use std::fs;
use std::io::Write;
use std::process::Command;

/// Errors from editor operations
#[derive(Debug, thiserror::Error)]
pub enum EditorError {
    #[error("Failed to create temp file: {0}")]
    TempFileError(#[from] std::io::Error),
    #[error("Editor command failed: {0}")]
    EditorFailed(String),
    #[error("No editor found. Set $EDITOR or $VISUAL environment variable")]
    NoEditorFound,
    #[error("Empty commit message")]
    EmptyMessage,
}

/// Trait for opening an editor - allows mocking in tests
pub trait Editor {
    /// Open editor with initial content, return the edited content.
    /// The comment_help is appended as commented lines (# prefix) for guidance.
    fn edit(&self, initial: &str, comment_help: &str) -> Result<String, EditorError>;
}

/// System editor implementation - uses $EDITOR, $VISUAL, or fallbacks
pub struct SystemEditor;

impl SystemEditor {
    pub fn new() -> Self {
        Self
    }

    /// Find the editor command to use
    fn find_editor() -> Result<String, EditorError> {
        // Try $EDITOR first, then $VISUAL, then fallbacks
        if let Ok(editor) = env::var("EDITOR") {
            return Ok(editor);
        }
        if let Ok(editor) = env::var("VISUAL") {
            return Ok(editor);
        }

        // Try common editors
        for editor in &["vim", "vi", "nano", "notepad"] {
            if Command::new("which")
                .arg(editor)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            {
                return Ok(editor.to_string());
            }
        }

        Err(EditorError::NoEditorFound)
    }
}

impl Default for SystemEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor for SystemEditor {
    fn edit(&self, initial: &str, comment_help: &str) -> Result<String, EditorError> {
        let editor = Self::find_editor()?;

        // Create temp file with initial content
        let mut temp_file = tempfile::Builder::new()
            .prefix("git-scramble-")
            .suffix(".txt")
            .tempfile()?;

        // Write initial content
        temp_file.write_all(initial.as_bytes())?;

        // Add comment help
        if !comment_help.is_empty() {
            temp_file.write_all(b"\n\n")?;
            for line in comment_help.lines() {
                temp_file.write_all(b"# ")?;
                temp_file.write_all(line.as_bytes())?;
                temp_file.write_all(b"\n")?;
            }
        }

        temp_file.flush()?;

        // Keep the file on disk but get ownership of the path
        // (NamedTempFile deletes on drop, so we use into_temp_path instead)
        let temp_path = temp_file.into_temp_path();

        // Parse editor command (might have args like "code --wait")
        let mut parts = editor.split_whitespace();
        let cmd = parts.next().unwrap();
        let args: Vec<&str> = parts.collect();

        let status = Command::new(cmd)
            .args(&args)
            .arg(&temp_path)
            .status()
            .map_err(|e| EditorError::EditorFailed(e.to_string()))?;

        if !status.success() {
            return Err(EditorError::EditorFailed(format!(
                "Editor exited with status: {}",
                status
            )));
        }

        // Read back the edited content
        let content = fs::read_to_string(&temp_path)?;

        // temp_path is dropped here, which deletes the file

        // Strip comment lines and trailing whitespace
        let cleaned = strip_comments(&content);

        if cleaned.trim().is_empty() {
            return Err(EditorError::EmptyMessage);
        }

        Ok(cleaned)
    }
}

/// Strip lines starting with # and normalize whitespace
fn strip_comments(content: &str) -> String {
    content
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_comments() {
        let input = "Title\n\nBody text\n# This is a comment\nMore body\n# Another comment";
        let expected = "Title\n\nBody text\nMore body";
        assert_eq!(strip_comments(input), expected);
    }

    #[test]
    fn test_strip_comments_empty() {
        let input = "# Just comments\n# More comments";
        assert_eq!(strip_comments(input), "");
    }
}
