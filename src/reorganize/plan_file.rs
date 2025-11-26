//! Plan file storage for resumable reorganization

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::{CommitDescription, Hunk, HunkId, PlannedChange, PlannedCommit};

/// Directory within .git for scramble state
const SCRAMBLE_DIR: &str = ".git/scramble";
/// Plan file name
const PLAN_FILE: &str = "plan.json";

/// Errors from plan file operations
#[derive(Debug, thiserror::Error)]
pub enum PlanFileError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(String),
    #[error("No saved plan found. Run 'git-scramble --plan-only' first.")]
    NoPlan,
}

/// A saved plan that can be resumed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPlan {
    /// Version for future compatibility
    pub version: u32,
    /// Strategy that was used to create this plan
    pub strategy: String,
    /// Base commit SHA that we reset to
    pub base_sha: String,
    /// Original HEAD before scramble
    pub original_head: String,
    /// The planned commits
    pub commits: Vec<SavedCommit>,
    /// Index of the next commit to create (for resumption)
    pub next_commit_index: usize,
    /// Any new hunks created by splitting (serialized)
    pub new_hunks: Vec<SavedHunk>,
    /// Hunks from the working tree (needed for apply/resume)
    pub working_tree_hunks: Vec<SavedHunk>,
    /// File to commits mapping (for new file staging)
    pub file_to_commits: Vec<(String, Vec<String>)>,
    /// New files to commits mapping
    pub new_files_to_commits: Vec<(String, Vec<String>)>,
}

/// A commit in the saved plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCommit {
    pub description: CommitDescription,
    pub changes: Vec<SavedChange>,
    /// SHA of the created commit (filled in after creation)
    pub created_sha: Option<String>,
}

/// A change specification that can be serialized
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SavedChange {
    #[serde(rename = "existing")]
    ExistingHunk { id: usize },
    #[serde(rename = "new")]
    NewHunk { id: usize },
}

/// A hunk that can be serialized (for new hunks created by splitting)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedHunk {
    pub id: usize,
    pub file_path: String,
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<SavedDiffLine>,
    pub likely_source_commits: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "content")]
pub enum SavedDiffLine {
    #[serde(rename = "context")]
    Context(String),
    #[serde(rename = "added")]
    Added(String),
    #[serde(rename = "removed")]
    Removed(String),
}

impl SavedPlan {
    /// Create a new saved plan from planned commits
    pub fn new(
        strategy: String,
        base_sha: String,
        original_head: String,
        planned_commits: &[PlannedCommit],
        working_tree_hunks: &[Hunk],
        new_hunks: &[Hunk],
        file_to_commits: &std::collections::HashMap<String, Vec<String>>,
        new_files_to_commits: &std::collections::HashMap<String, Vec<String>>,
    ) -> Self {
        let commits = planned_commits
            .iter()
            .map(|pc| SavedCommit {
                description: pc.description.clone(),
                changes: pc
                    .changes
                    .iter()
                    .map(|c| match c {
                        PlannedChange::ExistingHunk(id) => SavedChange::ExistingHunk { id: id.0 },
                        PlannedChange::NewHunk(h) => SavedChange::NewHunk { id: h.id.0 },
                    })
                    .collect(),
                created_sha: None,
            })
            .collect();

        let saved_new_hunks = new_hunks.iter().map(SavedHunk::from_hunk).collect();
        let saved_working_hunks = working_tree_hunks.iter().map(SavedHunk::from_hunk).collect();

        Self {
            version: 1,
            strategy,
            base_sha,
            original_head,
            commits,
            next_commit_index: 0,
            new_hunks: saved_new_hunks,
            working_tree_hunks: saved_working_hunks,
            file_to_commits: file_to_commits
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            new_files_to_commits: new_files_to_commits
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }

    /// Convert back to PlannedCommits
    pub fn to_planned_commits(&self) -> Vec<PlannedCommit> {
        self.commits
            .iter()
            .map(|sc| {
                let changes = sc
                    .changes
                    .iter()
                    .map(|c| match c {
                        SavedChange::ExistingHunk { id } => {
                            PlannedChange::ExistingHunk(HunkId(*id))
                        }
                        SavedChange::NewHunk { id } => {
                            // Find the new hunk in our saved hunks
                            let saved = self
                                .new_hunks
                                .iter()
                                .find(|h| h.id == *id)
                                .expect("New hunk not found in saved plan");
                            PlannedChange::NewHunk(saved.to_hunk())
                        }
                    })
                    .collect();

                PlannedCommit::new(sc.description.clone(), changes)
            })
            .collect()
    }

    /// Get the working tree hunks
    pub fn get_working_tree_hunks(&self) -> Vec<Hunk> {
        self.working_tree_hunks.iter().map(|h| h.to_hunk()).collect()
    }

    /// Get file_to_commits as a HashMap
    pub fn get_file_to_commits(&self) -> std::collections::HashMap<String, Vec<String>> {
        self.file_to_commits.iter().cloned().collect()
    }

    /// Get new_files_to_commits as a HashMap
    pub fn get_new_files_to_commits(&self) -> std::collections::HashMap<String, Vec<String>> {
        self.new_files_to_commits.iter().cloned().collect()
    }

    /// Get the remaining commits to create (for resumption)
    pub fn remaining_commits(&self) -> &[SavedCommit] {
        &self.commits[self.next_commit_index..]
    }

    /// Mark a commit as created
    pub fn mark_commit_created(&mut self, sha: String) {
        if self.next_commit_index < self.commits.len() {
            self.commits[self.next_commit_index].created_sha = Some(sha);
            self.next_commit_index += 1;
        }
    }

    /// Check if the plan is complete
    pub fn is_complete(&self) -> bool {
        self.next_commit_index >= self.commits.len()
    }
}

impl SavedHunk {
    pub fn from_hunk(hunk: &Hunk) -> Self {
        use crate::models::DiffLine;

        Self {
            id: hunk.id.0,
            file_path: hunk.file_path.to_string_lossy().to_string(),
            old_start: hunk.old_start,
            old_count: hunk.old_count,
            new_start: hunk.new_start,
            new_count: hunk.new_count,
            lines: hunk
                .lines
                .iter()
                .map(|l| match l {
                    DiffLine::Context(s) => SavedDiffLine::Context(s.clone()),
                    DiffLine::Added(s) => SavedDiffLine::Added(s.clone()),
                    DiffLine::Removed(s) => SavedDiffLine::Removed(s.clone()),
                })
                .collect(),
            likely_source_commits: hunk.likely_source_commits.clone(),
        }
    }

    pub fn to_hunk(&self) -> Hunk {
        use crate::models::DiffLine;

        Hunk {
            id: HunkId(self.id),
            file_path: PathBuf::from(&self.file_path),
            old_start: self.old_start,
            old_count: self.old_count,
            new_start: self.new_start,
            new_count: self.new_count,
            lines: self
                .lines
                .iter()
                .map(|l| match l {
                    SavedDiffLine::Context(s) => DiffLine::Context(s.clone()),
                    SavedDiffLine::Added(s) => DiffLine::Added(s.clone()),
                    SavedDiffLine::Removed(s) => DiffLine::Removed(s.clone()),
                })
                .collect(),
            likely_source_commits: self.likely_source_commits.clone(),
        }
    }
}

/// Get the plan file path
pub fn plan_file_path() -> PathBuf {
    PathBuf::from(SCRAMBLE_DIR).join(PLAN_FILE)
}

/// Save a plan to disk
pub fn save_plan(plan: &SavedPlan) -> Result<PathBuf, PlanFileError> {
    let dir = Path::new(SCRAMBLE_DIR);
    if !dir.exists() {
        fs::create_dir_all(dir)?;
    }

    let path = plan_file_path();
    let json = serde_json::to_string_pretty(plan)
        .map_err(|e| PlanFileError::Json(format!("Failed to serialize plan: {}", e)))?;

    fs::write(&path, json)?;
    Ok(path)
}

/// Load a plan from disk
pub fn load_plan() -> Result<SavedPlan, PlanFileError> {
    let path = plan_file_path();
    if !path.exists() {
        return Err(PlanFileError::NoPlan);
    }

    let json = fs::read_to_string(&path)?;
    let plan: SavedPlan = serde_json::from_str(&json)
        .map_err(|e| PlanFileError::Json(format!("Failed to parse plan: {}", e)))?;

    Ok(plan)
}

/// Check if a saved plan exists
pub fn has_saved_plan() -> bool {
    plan_file_path().exists()
}

/// Delete the saved plan
pub fn delete_plan() -> Result<(), PlanFileError> {
    let path = plan_file_path();
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DiffLine;
    use std::collections::HashMap;

    #[test]
    fn test_saved_plan_roundtrip() {
        let hunk = Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("test.rs"),
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 3,
            lines: vec![
                DiffLine::Context("context".to_string()),
                DiffLine::Added("added".to_string()),
            ],
            likely_source_commits: vec!["abc123".to_string()],
        };

        let planned = vec![PlannedCommit::new(
            CommitDescription::new("Test", "Test commit"),
            vec![
                PlannedChange::ExistingHunk(HunkId(0)),
                PlannedChange::NewHunk(hunk.clone()),
            ],
        )];

        let saved = SavedPlan::new(
            "preserve".to_string(),
            "base123".to_string(),
            "head456".to_string(),
            &planned,
            &[hunk.clone()],
            &[hunk],
            &HashMap::new(),
            &HashMap::new(),
        );

        let restored = saved.to_planned_commits();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].changes.len(), 2);
    }
}
