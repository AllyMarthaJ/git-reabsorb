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

        let mut hunks_by_commit: HashMap<&str, Vec<HunkId>> = HashMap::new();
        for hunk in hunks {
            let source_sha = source_commits
                .iter()
                .find(|sc| hunk.likely_source_commits.contains(&sc.sha))
                .map(|sc| sc.sha.as_str());

            if let Some(sha) = source_sha {
                hunks_by_commit.entry(sha).or_default().push(hunk.id);
            } else if let Some(first_likely) = hunk.likely_source_commits.first() {
                hunks_by_commit
                    .entry(first_likely.as_str())
                    .or_default()
                    .push(hunk.id);
            }
        }

        let mut planned = Vec::new();
        for source in source_commits {
            if let Some(hunk_ids) = hunks_by_commit.get(source.sha.as_str()) {
                planned.push(PlannedCommit::from_hunk_ids(
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
            likely_source_commits: vec![sha.to_string()],
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
        assert_eq!(planned[0].changes.len(), 2);
        assert_eq!(planned[1].description.short, "Second commit");
        assert_eq!(planned[1].changes.len(), 1);
    }
}
