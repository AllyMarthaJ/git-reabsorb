//! Unified diff patch generation.

use std::path::Path;

use crate::models::{ChangeType, DiffLine, FileChange, Hunk, HunkId};

pub struct PatchWriter;

impl PatchWriter {
    #[must_use]
    pub fn write_single_hunk(hunk: &Hunk) -> String {
        let change_type = ChangeType::from_hunk(hunk);
        Self::write_patch(&hunk.file_path, &[hunk], change_type)
    }

    #[must_use]
    pub fn write_multi_hunk(file_path: &Path, hunks: &[&Hunk]) -> String {
        let change_type = ChangeType::from_hunks(hunks);
        Self::write_patch(file_path, hunks, change_type)
    }

    #[must_use]
    pub fn write_patch<H: AsRef<Hunk>>(
        file_path: &Path,
        hunks: &[H],
        change_type: ChangeType,
    ) -> String {
        Self::write_patch_with_file_change(file_path, hunks, change_type, None)
    }

    #[must_use]
    pub fn write_patch_with_file_change<H: AsRef<Hunk>>(
        file_path: &Path,
        hunks: &[H],
        change_type: ChangeType,
        file_change: Option<&FileChange>,
    ) -> String {
        let mut patch = String::new();
        let path_str = file_path.to_string_lossy();

        if file_change.is_some() {
            patch.push_str(&format!("diff --git a/{} b/{}\n", path_str, path_str));
        }

        if let Some(mc) = file_change {
            match (&mc.old_mode, &mc.new_mode) {
                (None, Some(new)) => {
                    patch.push_str(&format!("new file mode {}\n", new));
                }
                (Some(old), None) => {
                    patch.push_str(&format!("deleted file mode {}\n", old));
                }
                (Some(old), Some(new)) => {
                    patch.push_str(&format!("old mode {}\n", old));
                    patch.push_str(&format!("new mode {}\n", new));
                }
                (None, None) => {}
            }
        }

        let (old_path, new_path) = match change_type {
            ChangeType::Added => ("/dev/null".to_string(), format!("b/{}", path_str)),
            ChangeType::Modified => (format!("a/{}", path_str), format!("b/{}", path_str)),
            ChangeType::Deleted => (format!("a/{}", path_str), "/dev/null".to_string()),
        };

        patch.push_str(&format!("--- {}\n", old_path));
        patch.push_str(&format!("+++ {}\n", new_path));

        for hunk in hunks {
            patch.push_str(&Self::write_hunk_body(hunk.as_ref()));
        }

        patch
    }

    #[must_use]
    pub fn write_hunk_body(hunk: &Hunk) -> String {
        let mut output = String::new();

        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
        ));

        let last_old_idx = hunk
            .lines
            .iter()
            .rposition(|line| matches!(line, DiffLine::Removed(_) | DiffLine::Context(_)));
        let last_new_idx = hunk
            .lines
            .iter()
            .rposition(|line| matches!(line, DiffLine::Added(_) | DiffLine::Context(_)));

        for (idx, line) in hunk.lines.iter().enumerate() {
            match line {
                DiffLine::Context(s) => {
                    output.push(' ');
                    output.push_str(s);
                    output.push('\n');
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

    #[must_use]
    pub fn create_new_file_hunk(file_path: &Path, hunks: &[&Hunk]) -> Hunk {
        let mut new_lines = Vec::new();

        for hunk in hunks {
            for line in &hunk.lines {
                match line {
                    DiffLine::Added(s) | DiffLine::Context(s) => {
                        new_lines.push(DiffLine::Added(s.clone()));
                    }
                    DiffLine::Removed(_) => {}
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

    #[must_use]
    pub fn create_delete_file_hunk(file_path: &Path, hunks: &[&Hunk]) -> Hunk {
        let mut removed_lines = Vec::new();

        for hunk in hunks {
            for line in &hunk.lines {
                match line {
                    DiffLine::Removed(s) | DiffLine::Context(s) => {
                        removed_lines.push(DiffLine::Removed(s.clone()));
                    }
                    DiffLine::Added(_) => {}
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

impl ChangeType {
    #[must_use]
    pub fn from_hunk(hunk: &Hunk) -> Self {
        if hunk.old_count == 0 && hunk.new_count > 0 {
            ChangeType::Added
        } else if hunk.old_count > 0 && hunk.new_count == 0 {
            ChangeType::Deleted
        } else {
            ChangeType::Modified
        }
    }

    #[must_use]
    pub fn from_hunks(hunks: &[&Hunk]) -> Self {
        hunks
            .first()
            .map(|h| Self::from_hunk(h))
            .unwrap_or(ChangeType::Modified)
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
        assert_eq!(ChangeType::from_hunk(&new), ChangeType::Added);

        let deleted = make_deleted_file_hunk();
        assert_eq!(ChangeType::from_hunk(&deleted), ChangeType::Deleted);

        let modified = make_simple_hunk();
        assert_eq!(ChangeType::from_hunk(&modified), ChangeType::Modified);
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
        let patch = PatchWriter::write_patch(&hunk.file_path, &[&hunk], ChangeType::Added);

        assert!(patch.contains("--- /dev/null"));
        assert!(patch.contains("+++ b/src/main.rs"));
    }

    #[test]
    fn test_create_new_file_hunk_from_modification() {
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

        assert_eq!(new_hunk.old_count, 0);
        assert_eq!(new_hunk.new_count, 2);
        assert_eq!(new_hunk.lines.len(), 2);

        assert!(matches!(&new_hunk.lines[0], DiffLine::Added(s) if s == "line1"));
        assert!(matches!(&new_hunk.lines[1], DiffLine::Added(s) if s == "new_line"));

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

        assert_eq!(new_hunk.old_count, 0);
        assert_eq!(new_hunk.new_count, 5);

        for line in &new_hunk.lines {
            assert!(matches!(line, DiffLine::Added(_)));
        }

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

        assert_eq!(patch.matches("--- a/src/main.rs").count(), 1);
        assert_eq!(patch.matches("+++ b/src/main.rs").count(), 1);
        assert!(patch.contains("@@ -1,2 +1,3 @@"));
        assert!(patch.contains("@@ -10,2 +11,3 @@"));
    }
}
