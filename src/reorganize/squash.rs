use crate::models::{CommitDescription, Hunk, PlannedCommit, SourceCommit};
use crate::reorganize::{ReorganizeError, Reorganizer};

/// Squashes all hunks into a single commit.
pub struct Squash;

impl Reorganizer for Squash {
    fn plan(
        &self,
        source_commits: &[SourceCommit],
        hunks: &[Hunk],
    ) -> Result<Vec<PlannedCommit>, ReorganizeError> {
        if hunks.is_empty() {
            return Err(ReorganizeError::NoHunks);
        }

        let hunk_ids: Vec<_> = hunks.iter().map(|h| h.id).collect();

        let short = if source_commits.len() == 1 {
            source_commits[0].message.short.clone()
        } else {
            format!("Squashed {} commits", source_commits.len())
        };

        let mut long = short.clone();
        if source_commits.len() > 1 {
            long.push_str("\n\nSquashed commits:\n");
            for commit in source_commits {
                long.push_str(&format!("- {}\n", commit.message.short));
            }
        }

        Ok(vec![PlannedCommit::from_hunk_ids(
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
    use crate::test_utils::{make_hunk, make_source_commit};

    #[test]
    fn test_squash() {
        let commits = vec![
            make_source_commit("abc", "First"),
            make_source_commit("def", "Second"),
        ];

        let hunks = vec![make_hunk(0), make_hunk(1), make_hunk(2)];

        let reorganizer = Squash;
        let planned = reorganizer.plan(&commits, &hunks).unwrap();

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].changes.len(), 3);
        assert!(planned[0].description.short.contains("Squashed"));
    }
}
