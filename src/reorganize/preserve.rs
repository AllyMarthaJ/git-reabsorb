use std::collections::HashMap;

use crate::models::{Hunk, HunkId, PlannedCommit, SourceCommit};
use crate::reorganize::{ReorganizeError, Reorganizer};

/// Preserves the original commit structure.
/// Each source commit becomes a planned commit with the same hunks.
pub struct PreserveOriginal;

impl Reorganizer for PreserveOriginal {
    fn plan(
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
                    source.message.clone(),
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
    use crate::test_utils::make_hunk_with_source;

    #[test]
    fn test_preserve_original() {
        let commits = vec![
            SourceCommit::new("abc", "First commit", "First commit\n\nDetails"),
            SourceCommit::new("def", "Second commit", "Second commit"),
        ];

        let hunks = vec![
            make_hunk_with_source(0, "test.rs", vec!["abc".to_string()]),
            make_hunk_with_source(1, "test.rs", vec!["abc".to_string()]),
            make_hunk_with_source(2, "test.rs", vec!["def".to_string()]),
        ];

        let reorganizer = PreserveOriginal;
        let planned = reorganizer.plan(&commits, &hunks).unwrap();

        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].description.short, "First commit");
        assert_eq!(planned[0].changes.len(), 2);
        assert_eq!(planned[1].description.short, "Second commit");
        assert_eq!(planned[1].changes.len(), 1);
    }
}
