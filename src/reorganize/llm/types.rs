//! Types for LLM-based reorganization

use serde::{Deserialize, Serialize};

/// Context about a source commit sent to the LLM
#[derive(Debug, Clone, Serialize)]
pub struct CommitContext {
    pub sha: String,
    pub message: String,
}

/// Context about a hunk sent to the LLM
#[derive(Debug, Clone, Serialize)]
pub struct HunkContext {
    pub id: usize,
    pub file_path: String,
    pub old_start: u32,
    pub new_start: u32,
    /// The actual diff content (+/- lines)
    pub diff_content: String,
    /// Which source commit this hunk came from
    pub source_commit_sha: Option<String>,
}

/// Full context sent to the LLM
#[derive(Debug, Clone, Serialize)]
pub struct LlmContext {
    pub source_commits: Vec<CommitContext>,
    pub hunks: Vec<HunkContext>,
}

/// A commit planned by the LLM
#[derive(Debug, Clone, Deserialize)]
pub struct LlmCommit {
    #[serde(flatten)]
    pub description: crate::models::CommitDescription,
    pub changes: Vec<ChangeSpec>,
}

/// Specification for a change in a commit
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ChangeSpec {
    /// Use an entire existing hunk
    #[serde(rename = "hunk")]
    Hunk { id: usize },

    /// Use specific lines from a hunk (for splitting)
    /// Lines are 1-indexed and refer to the diff lines (the +/- lines)
    #[serde(rename = "partial")]
    Partial { hunk_id: usize, lines: Vec<usize> },

    /// Raw diff content (for complex merges or LLM-generated changes)
    #[serde(rename = "raw")]
    Raw { file_path: String, diff: String },
}

/// The complete plan returned by the LLM
#[derive(Debug, Clone, Deserialize)]
pub struct LlmPlan {
    pub commits: Vec<LlmCommit>,
}
