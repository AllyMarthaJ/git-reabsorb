//! Plan persistence for resumable reorganization.
//!
//! This module handles saving and loading reorganization plans to disk,
//! enabling resumable operations across sessions.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::{CommitDescription, FileChange, Hunk, PlannedChange, PlannedCommit};

const REABSORB_DIR: &str = ".git/reabsorb";
const PLAN_FILE: &str = "plan.json";

/// Errors from plan file operations.
#[derive(Debug, thiserror::Error)]
pub enum PlanFileError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(String),
    #[error("No saved plan found. Run 'git reabsorb plan --save-plan' first.")]
    NoPlan,
}

/// A saved reorganization plan that can be resumed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPlan {
    pub version: u32,
    pub strategy: String,
    pub base_sha: String,
    pub original_head: String,
    pub commits: Vec<SavedCommit>,
    pub next_commit_index: usize,
    pub working_tree_hunks: Vec<Hunk>,
    pub file_to_commits: Vec<(String, Vec<String>)>,
    #[serde(default)]
    pub file_changes: Vec<FileChange>,
}

/// A single commit in a saved plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCommit {
    pub description: CommitDescription,
    pub changes: Vec<PlannedChange>,
    pub created_sha: Option<String>,
}

impl SavedPlan {
    pub fn new(
        strategy: String,
        base_sha: String,
        original_head: String,
        planned_commits: &[PlannedCommit],
        working_tree_hunks: &[Hunk],
        file_to_commits: &HashMap<String, Vec<String>>,
        file_changes: &[FileChange],
    ) -> Self {
        Self {
            version: 1,
            strategy,
            base_sha,
            original_head,
            commits: planned_commits.iter().map(SavedCommit::from).collect(),
            next_commit_index: 0,
            working_tree_hunks: working_tree_hunks.to_vec(),
            file_to_commits: file_to_commits
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            file_changes: file_changes.to_vec(),
        }
    }

    pub fn to_planned_commits(&self) -> Vec<PlannedCommit> {
        self.commits
            .iter()
            .map(|sc| PlannedCommit::new(sc.description.clone(), sc.changes.clone()))
            .collect()
    }

    pub fn get_working_tree_hunks(&self) -> Vec<Hunk> {
        self.working_tree_hunks.clone()
    }

    pub fn get_file_to_commits(&self) -> HashMap<String, Vec<String>> {
        self.file_to_commits.iter().cloned().collect()
    }

    pub fn get_file_changes(&self) -> Vec<FileChange> {
        self.file_changes.clone()
    }

    pub fn remaining_commits(&self) -> &[SavedCommit] {
        &self.commits[self.next_commit_index..]
    }

    pub fn mark_commit_created(&mut self, sha: String) {
        if self.next_commit_index < self.commits.len() {
            self.commits[self.next_commit_index].created_sha = Some(sha);
            self.next_commit_index += 1;
        }
    }

    pub fn is_complete(&self) -> bool {
        self.next_commit_index >= self.commits.len()
    }
}

impl From<&PlannedCommit> for SavedCommit {
    fn from(pc: &PlannedCommit) -> Self {
        Self {
            description: pc.description.clone(),
            changes: pc.changes.clone(),
            created_sha: None,
        }
    }
}

// --- Trait abstraction ---

/// Trait abstracting how plans are persisted between runs.
pub trait PlanStore {
    fn load(&self) -> Result<SavedPlan, PlanFileError>;
    fn save(&self, plan: &SavedPlan) -> Result<(), PlanFileError>;
    fn delete(&self) -> Result<(), PlanFileError>;
    fn exists(&self) -> bool;
}

/// Filesystem-backed plan store using `.git/reabsorb/plan.json`.
pub struct FilePlanStore {
    namespace: String,
}

impl FilePlanStore {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
        }
    }
}

impl Default for FilePlanStore {
    fn default() -> Self {
        Self::new("default")
    }
}

impl PlanStore for FilePlanStore {
    fn load(&self) -> Result<SavedPlan, PlanFileError> {
        load_plan(&self.namespace)
    }

    fn save(&self, plan: &SavedPlan) -> Result<(), PlanFileError> {
        save_plan(&self.namespace, plan).map(|_| ())
    }

    fn delete(&self) -> Result<(), PlanFileError> {
        delete_plan(&self.namespace)
    }

    fn exists(&self) -> bool {
        has_saved_plan(&self.namespace)
    }
}

// --- File operations ---

fn base_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = env::var("GIT_REABSORB_PLAN_DIR") {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(dir));
        }
    }
    dirs.push(PathBuf::from(REABSORB_DIR));
    dirs.push(PathBuf::from(".git-reabsorb"));
    dirs
}

fn namespace_dirs(namespace: &str) -> Vec<PathBuf> {
    base_dirs()
        .into_iter()
        .map(|dir| dir.join(namespace))
        .collect()
}

fn existing_plan_path(namespace: &str) -> Option<PathBuf> {
    for dir in namespace_dirs(namespace) {
        let path = dir.join(PLAN_FILE);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Get the path where a plan would be saved.
pub fn plan_file_path(namespace: &str) -> PathBuf {
    existing_plan_path(namespace)
        .unwrap_or_else(|| PathBuf::from(REABSORB_DIR).join(namespace).join(PLAN_FILE))
}

/// Save a plan to disk.
pub fn save_plan(namespace: &str, plan: &SavedPlan) -> Result<PathBuf, PlanFileError> {
    let json =
        serde_json::to_string_pretty(plan).map_err(|e| PlanFileError::Json(e.to_string()))?;
    let mut last_err: Option<std::io::Error> = None;

    for dir in namespace_dirs(namespace) {
        if let Err(e) = fs::create_dir_all(&dir) {
            last_err = Some(e);
            continue;
        }
        let path = dir.join(PLAN_FILE);
        match fs::write(&path, &json) {
            Ok(_) => return Ok(path),
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        }
    }

    Err(PlanFileError::Io(last_err.unwrap_or_else(|| {
        std::io::Error::other("Failed to save plan")
    })))
}

/// Load a plan from disk.
pub fn load_plan(namespace: &str) -> Result<SavedPlan, PlanFileError> {
    if let Some(path) = existing_plan_path(namespace) {
        let json = fs::read_to_string(&path)?;
        return serde_json::from_str(&json).map_err(|e| PlanFileError::Json(e.to_string()));
    }
    Err(PlanFileError::NoPlan)
}

/// Check if a saved plan exists.
pub fn has_saved_plan(namespace: &str) -> bool {
    existing_plan_path(namespace).is_some()
}

/// Delete a saved plan.
pub fn delete_plan(namespace: &str) -> Result<(), PlanFileError> {
    if let Some(path) = existing_plan_path(namespace) {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DiffLine, HunkId};

    fn test_hunk() -> Hunk {
        Hunk {
            id: HunkId(0),
            file_path: PathBuf::from("test.rs"),
            old_start: 1,
            old_count: 2,
            new_start: 1,
            new_count: 3,
            lines: vec![
                DiffLine::Context("ctx".into()),
                DiffLine::Added("add".into()),
            ],
            likely_source_commits: vec!["abc".into()],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        }
    }

    #[test]
    fn roundtrip() {
        let hunk = test_hunk();
        let planned = vec![PlannedCommit::new(
            CommitDescription::new("Test", "desc"),
            vec![
                PlannedChange::ExistingHunk(HunkId(0)),
                PlannedChange::NewHunk(hunk.clone()),
            ],
        )];

        let saved = SavedPlan::new(
            "preserve".into(),
            "base".into(),
            "head".into(),
            &planned,
            std::slice::from_ref(&hunk),
            &HashMap::new(),
            &[],
        );

        let restored = saved.to_planned_commits();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].changes.len(), 2);
    }
}
