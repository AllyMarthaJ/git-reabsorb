//! Patch context for determining correct headers during plan execution.

use std::collections::HashMap;
use std::path::Path;

use crate::models::{ChangeType, FileChange, Hunk};

use super::PatchWriter;

#[derive(Debug, Clone)]
pub struct PatchContext {
    file_changes: HashMap<String, FileChange>,
}

impl PatchContext {
    pub fn new(file_changes: &[FileChange]) -> Self {
        Self {
            file_changes: file_changes
                .iter()
                .map(|fc| (fc.file_path.to_string_lossy().to_string(), fc.clone()))
                .collect(),
        }
    }

    pub fn empty() -> Self {
        Self {
            file_changes: HashMap::new(),
        }
    }

    pub fn get_file_change(&self, file_path: &Path) -> Option<&FileChange> {
        let path_str = file_path.to_string_lossy();
        self.file_changes.get(path_str.as_ref())
    }

    pub fn is_new_in_range(&self, file_path: &Path) -> bool {
        let path_str = file_path.to_string_lossy();
        self.file_changes
            .get(path_str.as_ref())
            .is_some_and(|fc| fc.change_type == ChangeType::Added)
    }

    pub fn determine_change_type(
        &self,
        file_path: &Path,
        file_in_index: bool,
        hunks: &[&Hunk],
    ) -> ChangeType {
        let is_deletion = hunks.iter().all(|h| h.new_count == 0);

        if is_deletion && file_in_index {
            return ChangeType::Deleted;
        }

        if file_in_index {
            return ChangeType::Modified;
        }

        if self.is_new_in_range(file_path) {
            return ChangeType::Added;
        }

        let hunk_indicates_new = hunks.iter().all(|h| h.old_count == 0);
        if hunk_indicates_new {
            return ChangeType::Added;
        }

        ChangeType::Added
    }

    pub fn generate_patch(
        &self,
        file_path: &Path,
        hunks: &[&Hunk],
        file_in_index: bool,
    ) -> (String, ChangeType) {
        let change_type = self.determine_change_type(file_path, file_in_index, hunks);
        let file_change = self.get_file_change(file_path);

        let patch = match change_type {
            ChangeType::Added => {
                let new_file_hunk = PatchWriter::create_new_file_hunk(file_path, hunks);
                PatchWriter::write_patch_with_file_change(
                    file_path,
                    &[&new_file_hunk],
                    ChangeType::Added,
                    file_change,
                )
            }
            ChangeType::Deleted => {
                let delete_hunk = PatchWriter::create_delete_file_hunk(file_path, hunks);
                PatchWriter::write_patch_with_file_change(
                    file_path,
                    &[&delete_hunk],
                    ChangeType::Deleted,
                    file_change,
                )
            }
            ChangeType::Modified => PatchWriter::write_patch_with_file_change(
                file_path,
                hunks,
                ChangeType::Modified,
                file_change,
            ),
        };

        (patch, change_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DiffLine, HunkId};
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

    fn make_new_file_change(path: &str) -> FileChange {
        FileChange {
            file_path: PathBuf::from(path),
            change_type: ChangeType::Added,
            old_mode: None,
            new_mode: Some("100644".to_string()),
            is_binary: false,
            has_content_hunks: true,
            likely_source_commits: vec![],
        }
    }

    #[test]
    fn test_patch_context_empty() {
        let ctx = PatchContext::empty();
        assert!(!ctx.is_new_in_range(Path::new("any/file.rs")));
    }

    #[test]
    fn test_patch_context_with_new_files() {
        let file_changes = vec![
            make_new_file_change("src/new.rs"),
            make_new_file_change("src/other.rs"),
        ];
        let ctx = PatchContext::new(&file_changes);
        assert!(ctx.is_new_in_range(Path::new("src/new.rs")));
        assert!(ctx.is_new_in_range(Path::new("src/other.rs")));
        assert!(!ctx.is_new_in_range(Path::new("src/existing.rs")));
    }

    #[test]
    fn test_determine_change_type_existing_file() {
        let ctx = PatchContext::empty();
        let hunk = make_simple_hunk();

        let change_type = ctx.determine_change_type(Path::new("src/main.rs"), true, &[&hunk]);
        assert_eq!(change_type, ChangeType::Modified);
    }

    #[test]
    fn test_determine_change_type_new_file_by_hunk() {
        let ctx = PatchContext::empty();
        let hunk = make_new_file_hunk();

        let change_type = ctx.determine_change_type(Path::new("src/new.rs"), false, &[&hunk]);
        assert_eq!(change_type, ChangeType::Added);
    }

    #[test]
    fn test_determine_change_type_new_file_by_range() {
        let file_changes = vec![make_new_file_change("src/new.rs")];
        let ctx = PatchContext::new(&file_changes);
        let hunk = make_simple_hunk();

        let change_type = ctx.determine_change_type(Path::new("src/new.rs"), false, &[&hunk]);
        assert_eq!(change_type, ChangeType::Added);
    }

    #[test]
    fn test_determine_change_type_modification_hunk_no_file_becomes_new() {
        let ctx = PatchContext::empty();
        let hunk = make_simple_hunk();

        let change_type = ctx.determine_change_type(Path::new("src/main.rs"), false, &[&hunk]);
        assert_eq!(change_type, ChangeType::Added);
    }

    #[test]
    fn test_determine_change_type_deletion() {
        let ctx = PatchContext::empty();
        let hunk = make_deleted_file_hunk();

        let change_type = ctx.determine_change_type(Path::new("src/old.rs"), true, &[&hunk]);
        assert_eq!(change_type, ChangeType::Deleted);
    }

    #[test]
    fn test_generate_patch_transforms_modification_to_new_file() {
        let file_changes = vec![make_new_file_change("src/new.rs")];
        let ctx = PatchContext::new(&file_changes);

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

        let (patch, change_type) = ctx.generate_patch(Path::new("src/new.rs"), &[&hunk], false);

        assert_eq!(change_type, ChangeType::Added);
        assert!(patch.contains("--- /dev/null"));
        assert!(patch.contains("+++ b/src/new.rs"));
        assert!(patch.contains("+line1"));
        assert!(patch.contains("+new_line2"));
        assert!(!patch.contains("old_line2"));
    }

    #[test]
    fn test_generate_patch_deletion() {
        let ctx = PatchContext::empty();
        let hunk = make_deleted_file_hunk();

        let (patch, change_type) = ctx.generate_patch(Path::new("src/old.rs"), &[&hunk], true);

        assert_eq!(change_type, ChangeType::Deleted);
        assert!(patch.contains("--- a/src/old.rs"));
        assert!(patch.contains("+++ /dev/null"));
    }

    #[test]
    fn test_generate_patch_modification() {
        let ctx = PatchContext::empty();
        let hunk = make_simple_hunk();

        let (patch, change_type) = ctx.generate_patch(Path::new("src/main.rs"), &[&hunk], true);

        assert_eq!(change_type, ChangeType::Modified);
        assert!(patch.contains("--- a/src/main.rs"));
        assert!(patch.contains("+++ b/src/main.rs"));
        assert!(patch.contains("@@ -1,3 +1,4 @@"));
    }
}
