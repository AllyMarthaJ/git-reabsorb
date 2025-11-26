//! Plan file storage for resumable reorganization

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::{CommitDescription, DiffLine, Hunk, HunkId, PlannedChange, PlannedCommit};

const SCRAMBLE_DIR: &str = ".git/scramble";
const PLAN_FILE: &str = "plan.json";

#[derive(Debug, thiserror::Error)]
pub enum PlanFileError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(String),
    #[error("No saved plan found. Run 'git-scramble --plan-only' first.")]
    NoPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPlan {
    pub version: u32,
    pub strategy: String,
    pub base_sha: String,
    pub original_head: String,
    pub commits: Vec<SavedCommit>,
    pub next_commit_index: usize,
    pub new_hunks: Vec<SavedHunk>,
    pub working_tree_hunks: Vec<SavedHunk>,
    pub file_to_commits: Vec<(String, Vec<String>)>,
    pub new_files_to_commits: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCommit {
    pub description: CommitDescription,
    pub changes: Vec<SavedChange>,
    pub created_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SavedChange {
    #[serde(rename = "existing")]
    ExistingHunk { id: usize },
    #[serde(rename = "new")]
    NewHunk { id: usize },
}

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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        strategy: String,
        base_sha: String,
        original_head: String,
        planned_commits: &[PlannedCommit],
        working_tree_hunks: &[Hunk],
        new_hunks: &[Hunk],
        file_to_commits: &HashMap<String, Vec<String>>,
        new_files_to_commits: &HashMap<String, Vec<String>>,
    ) -> Self {
        Self {
            version: 1,
            strategy,
            base_sha,
            original_head,
            commits: planned_commits.iter().map(SavedCommit::from).collect(),
            next_commit_index: 0,
            new_hunks: new_hunks.iter().map(SavedHunk::from).collect(),
            working_tree_hunks: working_tree_hunks.iter().map(SavedHunk::from).collect(),
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
                            let saved = self
                                .new_hunks
                                .iter()
                                .find(|h| h.id == *id)
                                .expect("new hunk missing");
                            PlannedChange::NewHunk(Hunk::from(saved))
                        }
                    })
                    .collect();
                PlannedCommit::new(sc.description.clone(), changes)
            })
            .collect()
    }

    pub fn get_working_tree_hunks(&self) -> Vec<Hunk> {
        self.working_tree_hunks.iter().map(Hunk::from).collect()
    }

    pub fn get_file_to_commits(&self) -> HashMap<String, Vec<String>> {
        self.file_to_commits.iter().cloned().collect()
    }

    pub fn get_new_files_to_commits(&self) -> HashMap<String, Vec<String>> {
        self.new_files_to_commits.iter().cloned().collect()
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
            changes: pc
                .changes
                .iter()
                .map(|c| match c {
                    PlannedChange::ExistingHunk(id) => SavedChange::ExistingHunk { id: id.0 },
                    PlannedChange::NewHunk(h) => SavedChange::NewHunk { id: h.id.0 },
                })
                .collect(),
            created_sha: None,
        }
    }
}

impl From<&Hunk> for SavedHunk {
    fn from(hunk: &Hunk) -> Self {
        Self {
            id: hunk.id.0,
            file_path: hunk.file_path.to_string_lossy().to_string(),
            old_start: hunk.old_start,
            old_count: hunk.old_count,
            new_start: hunk.new_start,
            new_count: hunk.new_count,
            lines: hunk.lines.iter().map(SavedDiffLine::from).collect(),
            likely_source_commits: hunk.likely_source_commits.clone(),
        }
    }
}

impl From<&SavedHunk> for Hunk {
    fn from(saved: &SavedHunk) -> Self {
        Self {
            id: HunkId(saved.id),
            file_path: PathBuf::from(&saved.file_path),
            old_start: saved.old_start,
            old_count: saved.old_count,
            new_start: saved.new_start,
            new_count: saved.new_count,
            lines: saved.lines.iter().map(DiffLine::from).collect(),
            likely_source_commits: saved.likely_source_commits.clone(),
        }
    }
}

impl From<&DiffLine> for SavedDiffLine {
    fn from(line: &DiffLine) -> Self {
        match line {
            DiffLine::Context(s) => SavedDiffLine::Context(s.clone()),
            DiffLine::Added(s) => SavedDiffLine::Added(s.clone()),
            DiffLine::Removed(s) => SavedDiffLine::Removed(s.clone()),
        }
    }
}

impl From<&SavedDiffLine> for DiffLine {
    fn from(line: &SavedDiffLine) -> Self {
        match line {
            SavedDiffLine::Context(s) => DiffLine::Context(s.clone()),
            SavedDiffLine::Added(s) => DiffLine::Added(s.clone()),
            SavedDiffLine::Removed(s) => DiffLine::Removed(s.clone()),
        }
    }
}

fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = env::var("GIT_SCRAMBLE_PLAN_DIR") {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(dir));
        }
    }
    dirs.push(PathBuf::from(SCRAMBLE_DIR));
    dirs.push(PathBuf::from(".git-scramble"));
    dirs
}

fn existing_plan_path() -> Option<PathBuf> {
    for dir in candidate_dirs() {
        let path = dir.join(PLAN_FILE);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

pub fn plan_file_path() -> PathBuf {
    existing_plan_path().unwrap_or_else(|| PathBuf::from(SCRAMBLE_DIR).join(PLAN_FILE))
}

pub fn save_plan(plan: &SavedPlan) -> Result<PathBuf, PlanFileError> {
    let json =
        serde_json::to_string_pretty(plan).map_err(|e| PlanFileError::Json(e.to_string()))?;
    let mut last_err: Option<std::io::Error> = None;

    for dir in candidate_dirs() {
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
        std::io::Error::new(std::io::ErrorKind::Other, "Failed to save plan")
    })))
}

pub fn load_plan() -> Result<SavedPlan, PlanFileError> {
    if let Some(path) = existing_plan_path() {
        let json = fs::read_to_string(&path)?;
        return serde_json::from_str(&json).map_err(|e| PlanFileError::Json(e.to_string()));
    }
    Err(PlanFileError::NoPlan)
}

pub fn has_saved_plan() -> bool {
    existing_plan_path().is_some()
}

pub fn delete_plan() -> Result<(), PlanFileError> {
    if let Some(path) = existing_plan_path() {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
