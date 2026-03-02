//! Commit extraction to NDJSON with enriched data (diffs, stats, file lists).

use std::io::Write;

use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

use crate::assessment::criteria::DiffStats;
use crate::git::GitOps;
use crate::models::SourceCommit;

/// An extracted commit record with diff and metadata, suitable for NDJSON serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedCommit {
    pub hash: String,
    pub subject: String,
    pub body: String,
    pub author_name: String,
    pub author_date: String,
    pub diff: String,
    pub diff_stat: DiffStats,
    pub files_changed: Vec<String>,
    pub diff_truncated: bool,
}

/// Configuration for extraction.
pub struct ExtractConfig {
    /// Maximum diff size in bytes before truncation.
    pub max_diff_size: usize,
    /// Skip commits touching more than this many files.
    pub max_files: usize,
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self {
            max_diff_size: 50_000,
            max_files: 50,
        }
    }
}

/// Extract commits from a git range, enriching with diffs and stats.
///
/// Writes NDJSON (one JSON object per line) to the provided writer.
/// Returns the number of commits extracted.
pub fn extract_range<G: GitOps, W: Write>(
    git: &G,
    base: &str,
    head: &str,
    config: &ExtractConfig,
    writer: &mut W,
) -> Result<usize, ExtractError> {
    let commits = git
        .read_commits(base, head)
        .map_err(|e| ExtractError::Git(e.to_string()))?;

    info!("Extracting {} commits...", commits.len());

    let mut count = 0;
    for (i, commit) in commits.iter().enumerate() {
        match extract_one(git, commit, config) {
            Ok(Some(record)) => {
                let json = serde_json::to_string(&record)
                    .map_err(|e| ExtractError::Serialization(e.to_string()))?;
                writeln!(writer, "{}", json)
                    .map_err(|e| ExtractError::Io(e.to_string()))?;
                count += 1;
            }
            Ok(None) => {
                debug!(
                    "[{}/{}] Skipped {} (too many files)",
                    i + 1,
                    commits.len(),
                    &commit.sha[..8.min(commit.sha.len())]
                );
            }
            Err(e) => {
                warn!(
                    "[{}/{}] Failed to extract {}: {}",
                    i + 1,
                    commits.len(),
                    &commit.sha[..8.min(commit.sha.len())],
                    e
                );
            }
        }

        if (i + 1) % 500 == 0 {
            info!("  Extracted {}/{} commits...", i + 1, commits.len());
        }
    }

    Ok(count)
}

/// Extract a single commit. Returns None if the commit should be skipped.
fn extract_one<G: GitOps>(
    git: &G,
    commit: &SourceCommit,
    config: &ExtractConfig,
) -> Result<Option<ExtractedCommit>, ExtractError> {
    // Get files changed
    let files_changed = git
        .get_files_changed_in_commit(&commit.sha)
        .map_err(|e| ExtractError::Git(e.to_string()))?;

    // Skip if too many files
    if files_changed.len() > config.max_files {
        return Ok(None);
    }

    // Get diff via hunks (reuses existing infrastructure)
    let hunks = git
        .read_hunks(&commit.sha, 0)
        .map_err(|e| ExtractError::Git(e.to_string()))?;

    let mut diff = hunks
        .iter()
        .map(|h| h.to_patch())
        .collect::<Vec<_>>()
        .join("\n");

    // Compute stats from hunks
    let mut lines_added = 0usize;
    let mut lines_removed = 0usize;
    for hunk in &hunks {
        for line in &hunk.lines {
            match line {
                crate::models::DiffLine::Added(_) => lines_added += 1,
                crate::models::DiffLine::Removed(_) => lines_removed += 1,
                crate::models::DiffLine::Context(_) => {}
            }
        }
    }

    // Truncate diff if too large
    let diff_truncated = diff.len() > config.max_diff_size;
    if diff_truncated {
        // Truncate at a line boundary
        if let Some(pos) = diff[..config.max_diff_size].rfind('\n') {
            diff.truncate(pos);
        } else {
            diff.truncate(config.max_diff_size);
        }
    }

    // Split message into subject and body
    let subject = commit.message.short.clone();
    let body = if commit.message.long.len() > commit.message.short.len() {
        commit.message.long[commit.message.short.len()..]
            .trim()
            .to_string()
    } else {
        String::new()
    };

    Ok(Some(ExtractedCommit {
        hash: commit.sha.clone(),
        subject,
        body,
        author_name: String::new(), // SourceCommit doesn't carry author info; populated from git log
        author_date: String::new(),
        diff,
        diff_stat: DiffStats {
            files_changed: files_changed.len(),
            lines_added,
            lines_removed,
        },
        files_changed,
        diff_truncated,
    }))
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("Git error: {0}")]
    Git(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("IO error: {0}")]
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_config_defaults() {
        let config = ExtractConfig::default();
        assert_eq!(config.max_diff_size, 50_000);
        assert_eq!(config.max_files, 50);
    }

    #[test]
    fn extracted_commit_serializes() {
        let record = ExtractedCommit {
            hash: "abc123".to_string(),
            subject: "Fix auth bug".to_string(),
            body: "The auth token was expiring early.".to_string(),
            author_name: "Alice".to_string(),
            author_date: "2025-01-01T00:00:00Z".to_string(),
            diff: "+fixed line".to_string(),
            diff_stat: DiffStats {
                files_changed: 1,
                lines_added: 1,
                lines_removed: 0,
            },
            files_changed: vec!["src/auth.rs".to_string()],
            diff_truncated: false,
        };

        let json = serde_json::to_string(&record).unwrap();
        let restored: ExtractedCommit = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.hash, "abc123");
        assert_eq!(restored.diff_stat.lines_added, 1);
        assert!(!restored.diff_truncated);
    }
}
