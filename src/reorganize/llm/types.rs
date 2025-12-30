//! Types for LLM-based reorganization

use serde::{Deserialize, Serialize};

/// Context about a source commit sent to the LLM
#[derive(Debug, Clone, Serialize)]
pub struct CommitContext {
    pub source_commit: crate::models::SourceCommit,
    // Any more metadata that we need goes here.
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
    /// Which source commits this hunk likely came from
    pub source_commit_shas: Vec<String>,
}

/// Full context sent to the LLM
#[derive(Debug, Clone, Serialize)]
pub struct LlmContext {
    pub source_commits: Vec<CommitContext>,
    pub hunks: Vec<HunkContext>,
}

/// A commit planned by the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmCommit {
    #[serde(flatten)]
    pub description: crate::models::CommitDescription,
    pub changes: Vec<ChangeSpec>,
}

/// Specification for a change in a commit
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Response for fixing unassigned hunks
#[derive(Debug, Clone, Deserialize)]
pub struct FixUnassignedResponse {
    pub assignments: Vec<HunkAssignment>,
}

/// A single hunk assignment decision
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action")]
pub enum HunkAssignment {
    /// Add hunk to an existing commit
    #[serde(rename = "add_to_existing")]
    AddToExisting {
        hunk_id: usize,
        commit_description: String,
    },
    /// Create a new commit for this hunk
    #[serde(rename = "new_commit")]
    NewCommit {
        hunk_id: usize,
        short_description: String,
        long_description: String,
    },
}

/// Response for fixing duplicate hunk assignments
#[derive(Debug, Clone, Deserialize)]
pub struct FixDuplicateResponse {
    #[allow(dead_code)]
    pub hunk_id: usize,
    pub chosen_commit_index: usize,
}

/// Response for fixing overlapping hunk assignments
///
/// When two hunks have overlapping line ranges and are in different commits,
/// the LLM decides which commit should contain both hunks.
#[derive(Debug, Clone, Deserialize)]
pub struct FixOverlappingResponse {
    /// The hunk IDs that overlap
    #[allow(dead_code)]
    pub hunk_a: usize,
    #[allow(dead_code)]
    pub hunk_b: usize,
    /// The commit index that should contain both overlapping hunks
    pub chosen_commit_index: usize,
}

/// Response for fixing a commit message based on assessment feedback
#[derive(Debug, Clone, Deserialize)]
pub struct FixMessageResponse {
    /// The improved commit description
    pub description: crate::models::CommitDescription,
}
