use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::models::{CommitDescription, Hunk, HunkId, PlannedCommit, SourceCommit};
use crate::reorganize::{ReorganizeError, Reorganizer};

/// Groups hunks by file path.
/// Creates one commit per file with all changes to that file.
pub struct GroupByFile;

impl Reorganizer for GroupByFile {
    fn plan(
        &self,
        _source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        if hunks.is_empty() {
            return Err(ReorganizeError::NoHunks);
        }

        let mut hunks_by_file: BTreeMap<&PathBuf, Vec<HunkId>> = BTreeMap::new();
        for hunk in hunks {
            hunks_by_file
                .entry(&hunk.file_path)
                .or_default()
                .push(hunk.id);
        }

        let mut planned = Vec::new();
        for (file_path, hunk_ids) in hunks_by_file {
            let file_name = file_path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| file_path.to_string_lossy().to_string());

            let short = format!("Update {}", file_name);
            let long = format!("Update {}\n\nChanges to {}", file_name, file_path.display());

            planned.push(PlannedCommit::from_hunk_ids(
                CommitDescription::new(short, long),
                hunk_ids,
            ));
        }

        Ok(planned)
    }

    fn name(&self) -> &'static str {
        "by-file"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{make_hunk_in_file, make_source_commit};

    #[test]
    fn test_group_by_file() {
        let commits = vec![make_source_commit("abc", "Original")];

        let hunks = vec![
            make_hunk_in_file(0, "src/main.rs"),
            make_hunk_in_file(1, "src/lib.rs"),
            make_hunk_in_file(2, "src/main.rs"),
            make_hunk_in_file(3, "tests/test.rs"),
        ];

        let reorganizer = GroupByFile;
        let planned = reorganizer.plan(&commits, &hunks).unwrap();

        assert_eq!(planned.len(), 3);

        // Find main.rs commit
        let main_commit = planned
            .iter()
            .find(|p| p.description.short.contains("main.rs"))
            .unwrap();
        assert_eq!(main_commit.changes.len(), 2);
    }
}
