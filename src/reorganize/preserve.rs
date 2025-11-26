use std::collections::HashMap;

use crate::models::{CommitDescription, Hunk, HunkId, PlannedCommit, SourceCommit};
use crate::reorganize::{ReorganizeError, Reorganizer};

/// Preserves the original commit structure.
/// Each source commit becomes a planned commit with the same hunks.
pub struct PreserveOriginal;

impl Reorganizer for PreserveOriginal {
    fn reorganize(
        &self,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        if hunks.is_empty() {
            return Err(ReorganizeError::NoHunks);
        }

        // Group hunks by source commit SHA
        let mut hunks_by_commit: HashMap<&str, Vec<HunkId>> = HashMap::new();
        for hunk in hunks {
            hunks_by_commit
                .entry(&hunk.source_commit_sha)
                .or_default()
                .push(hunk.id);
        }

        // Create planned commits in original order
        let mut planned = Vec::new();
        for source in source_commits {
            if let Some(hunk_ids) = hunks_by_commit.get(source.sha.as_str()) {
                planned.push(PlannedCommit::new(
                    CommitDescription::new(
                        source.short_description.clone(),
                        source.long_description.clone(),
                    ),
                    hunk_ids.clone(),
                ));
            }
        }

        Ok(planned)
    }

    fn name(&self) -> &'static str {
        "preserve"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DiffLine;
    use std::path::PathBuf;

    fn make_hunk(id: usize, sha: &str) -> Hunk {
        Hunk {
            id: HunkId(id),
            file_path: PathBuf::from("test.rs"),
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            lines: vec![DiffLine::Added("test".to_string())],
            source_commit_sha: sha.to_string(),
        }
    }

    #[test]
    fn test_preserve_original() {
        let commits = vec![
            SourceCommit {
                sha: "abc".to_string(),
                short_description: "First commit".to_string(),
                long_description: "First commit\n\nDetails".to_string(),
            },
            SourceCommit {
                sha: "def".to_string(),
                short_description: "Second commit".to_string(),
                long_description: "Second commit".to_string(),
            },
        ];

        let hunks = vec![
            make_hunk(0, "abc"),
            make_hunk(1, "abc"),
            make_hunk(2, "def"),
        ];

        let reorganizer = PreserveOriginal;
        let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].description.short, "First commit");
        assert_eq!(planned[0].hunk_ids.len(), 2);
        assert_eq!(planned[1].description.short, "Second commit");
        assert_eq!(planned[1].hunk_ids.len(), 1);
    }
}
