use crate::models::{CommitDescription, Hunk, PlannedCommit, SourceCommit};
use crate::reorganize::{ReorganizeError, Reorganizer};

/// Squashes all hunks into a single commit.
pub struct Squash;

impl Reorganizer for Squash {
    fn reorganize(
        &self,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        if hunks.is_empty() {
            return Err(ReorganizeError::NoHunks);
        }

        // Collect all hunk IDs
        let hunk_ids: Vec<_> = hunks.iter().map(|h| h.id).collect();

        // Build description from all source commits
        let short = if source_commits.len() == 1 {
            source_commits[0].short_description.clone()
        } else {
            format!("Squashed {} commits", source_commits.len())
        };

        let mut long = short.clone();
        if source_commits.len() > 1 {
            long.push_str("\n\nSquashed commits:\n");
            for commit in source_commits {
                long.push_str(&format!("- {}\n", commit.short_description));
            }
        }

        Ok(vec![PlannedCommit::new(
            CommitDescription::new(short, long),
            hunk_ids,
        )])
    }

    fn name(&self) -> &'static str {
        "squash"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DiffLine, HunkId};
    use std::path::PathBuf;

    fn make_hunk(id: usize) -> Hunk {
        Hunk {
            id: HunkId(id),
            file_path: PathBuf::from("test.rs"),
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            lines: vec![DiffLine::Added("test".to_string())],
            source_commit_sha: "abc".to_string(),
        }
    }

    #[test]
    fn test_squash() {
        let commits = vec![
            SourceCommit {
                sha: "abc".to_string(),
                short_description: "First".to_string(),
                long_description: "First".to_string(),
            },
            SourceCommit {
                sha: "def".to_string(),
                short_description: "Second".to_string(),
                long_description: "Second".to_string(),
            },
        ];

        let hunks = vec![make_hunk(0), make_hunk(1), make_hunk(2)];

        let reorganizer = Squash;
        let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].hunk_ids.len(), 3);
        assert!(planned[0].description.short.contains("Squashed"));
    }
}
