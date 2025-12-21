//! Plan validation and repair for reorganization strategies
//!
//! This module provides unified validation for planned commits, regardless of
//! which reorganization strategy produced them.

use std::collections::{HashMap, HashSet};

use crate::models::{Hunk, HunkId, PlannedChange, PlannedCommit, PlannedCommitId};

/// Issues that can occur in a reorganization plan
#[derive(Debug, Clone)]
pub enum ValidationIssue {
    /// A commit has an empty message
    EmptyMessage { commit_id: PlannedCommitId },

    /// A commit message is too long
    MessageTooLong {
        commit_id: PlannedCommitId,
        length: usize,
    },

    /// A commit has no changes
    NoChanges { commit_id: PlannedCommitId },

    /// A hunk reference is invalid (doesn't exist in the hunk list)
    InvalidHunk {
        commit_id: PlannedCommitId,
        hunk_id: HunkId,
    },

    /// A hunk appears multiple times within the same commit
    DuplicateHunkInCommit {
        commit_id: PlannedCommitId,
        hunk_id: HunkId,
    },

    /// A hunk is assigned to multiple commits
    DuplicateHunkAcrossCommits {
        hunk_id: HunkId,
        commit_ids: Vec<PlannedCommitId>,
    },

    /// Hunks that were not assigned to any commit
    UnassignedHunks { hunk_ids: Vec<HunkId> },

    /// A dependency references a non-existent commit
    InvalidDependency {
        commit_id: PlannedCommitId,
        missing_dependency: PlannedCommitId,
    },

    /// Cyclic dependency detected among commits
    CyclicDependency { commit_ids: Vec<PlannedCommitId> },
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMessage { commit_id } => {
                write!(f, "{}: empty commit message", commit_id)
            }
            Self::MessageTooLong { commit_id, length } => {
                write!(
                    f,
                    "{}: message too long ({} chars, max 72)",
                    commit_id, length
                )
            }
            Self::NoChanges { commit_id } => {
                write!(f, "{}: no changes in commit", commit_id)
            }
            Self::InvalidHunk { commit_id, hunk_id } => {
                write!(f, "{}: invalid hunk reference {}", commit_id, hunk_id)
            }
            Self::DuplicateHunkInCommit { commit_id, hunk_id } => {
                write!(f, "{}: duplicate hunk {} within commit", commit_id, hunk_id)
            }
            Self::DuplicateHunkAcrossCommits { hunk_id, commit_ids } => {
                let commits: Vec<_> = commit_ids.iter().map(|c| c.to_string()).collect();
                write!(
                    f,
                    "{} assigned to multiple commits: {}",
                    hunk_id,
                    commits.join(", ")
                )
            }
            Self::UnassignedHunks { hunk_ids } => {
                let hunks: Vec<_> = hunk_ids.iter().map(|h| h.to_string()).collect();
                write!(f, "unassigned hunks: {}", hunks.join(", "))
            }
            Self::InvalidDependency {
                commit_id,
                missing_dependency,
            } => {
                write!(
                    f,
                    "{}: depends on non-existent {}",
                    commit_id, missing_dependency
                )
            }
            Self::CyclicDependency { commit_ids } => {
                let commits: Vec<_> = commit_ids.iter().map(|c| c.to_string()).collect();
                write!(f, "cyclic dependency: {}", commits.join(" -> "))
            }
        }
    }
}

/// Result of validating a plan
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub issues: Vec<ValidationIssue>,
}

impl ValidationResult {
    pub fn is_valid(&self) -> bool {
        self.issues.is_empty()
    }

    /// Check if there are any fixable issues (unassigned hunks, duplicates)
    pub fn has_fixable_issues(&self) -> bool {
        self.issues.iter().any(|issue| {
            matches!(
                issue,
                ValidationIssue::UnassignedHunks { .. }
                    | ValidationIssue::DuplicateHunkAcrossCommits { .. }
                    | ValidationIssue::DuplicateHunkInCommit { .. }
            )
        })
    }

    /// Get unassigned hunk IDs if any
    pub fn unassigned_hunks(&self) -> Option<&[HunkId]> {
        self.issues.iter().find_map(|issue| {
            if let ValidationIssue::UnassignedHunks { hunk_ids } = issue {
                Some(hunk_ids.as_slice())
            } else {
                None
            }
        })
    }

    /// Get duplicate hunk assignments if any
    pub fn duplicate_hunks(&self) -> Vec<(HunkId, Vec<PlannedCommitId>)> {
        self.issues
            .iter()
            .filter_map(|issue| {
                if let ValidationIssue::DuplicateHunkAcrossCommits { hunk_id, commit_ids } = issue {
                    Some((*hunk_id, commit_ids.clone()))
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Validate a reorganization plan
pub fn validate_plan(commits: &[PlannedCommit], hunks: &[Hunk]) -> ValidationResult {
    let mut issues = Vec::new();
    let valid_hunk_ids: HashSet<HunkId> = hunks.iter().map(|h| h.id).collect();
    let valid_commit_ids: HashSet<PlannedCommitId> = commits.iter().map(|c| c.id).collect();

    // Track hunk assignments for duplicate detection
    let mut hunk_assignments: HashMap<HunkId, Vec<PlannedCommitId>> = HashMap::new();

    for commit in commits {
        // Check for empty message
        if commit.description.short.trim().is_empty() {
            issues.push(ValidationIssue::EmptyMessage {
                commit_id: commit.id,
            });
        }

        // Check for message length
        if commit.description.short.len() > 72 {
            issues.push(ValidationIssue::MessageTooLong {
                commit_id: commit.id,
                length: commit.description.short.len(),
            });
        }

        // Check for no changes
        if commit.changes.is_empty() {
            issues.push(ValidationIssue::NoChanges {
                commit_id: commit.id,
            });
        }

        // Check hunks within this commit
        let mut seen_in_commit: HashSet<HunkId> = HashSet::new();
        for change in &commit.changes {
            if let PlannedChange::ExistingHunk(hunk_id) = change {
                // Check if hunk is valid
                if !valid_hunk_ids.contains(hunk_id) {
                    issues.push(ValidationIssue::InvalidHunk {
                        commit_id: commit.id,
                        hunk_id: *hunk_id,
                    });
                }

                // Check for duplicate within commit
                if !seen_in_commit.insert(*hunk_id) {
                    issues.push(ValidationIssue::DuplicateHunkInCommit {
                        commit_id: commit.id,
                        hunk_id: *hunk_id,
                    });
                }

                // Track for cross-commit duplicate detection
                hunk_assignments
                    .entry(*hunk_id)
                    .or_default()
                    .push(commit.id);
            }
        }

        // Check dependencies
        for dep_id in &commit.depends_on {
            if !valid_commit_ids.contains(dep_id) {
                issues.push(ValidationIssue::InvalidDependency {
                    commit_id: commit.id,
                    missing_dependency: *dep_id,
                });
            }
        }
    }

    // Check for hunks assigned to multiple commits
    for (hunk_id, commit_ids) in &hunk_assignments {
        if commit_ids.len() > 1 {
            issues.push(ValidationIssue::DuplicateHunkAcrossCommits {
                hunk_id: *hunk_id,
                commit_ids: commit_ids.clone(),
            });
        }
    }

    // Check for unassigned hunks
    let assigned_hunks: HashSet<HunkId> = hunk_assignments.keys().copied().collect();
    let unassigned: Vec<HunkId> = valid_hunk_ids
        .difference(&assigned_hunks)
        .copied()
        .collect();
    if !unassigned.is_empty() {
        issues.push(ValidationIssue::UnassignedHunks { hunk_ids: unassigned });
    }

    // Check for cyclic dependencies
    if let Some(cycle) = detect_cycle(commits) {
        issues.push(ValidationIssue::CyclicDependency { commit_ids: cycle });
    }

    ValidationResult { issues }
}

/// Detect cyclic dependencies using DFS
fn detect_cycle(commits: &[PlannedCommit]) -> Option<Vec<PlannedCommitId>> {
    let mut visited = HashSet::new();
    let mut rec_stack = HashSet::new();
    let mut path = Vec::new();

    for commit in commits {
        if !visited.contains(&commit.id) {
            if let Some(cycle) = dfs_cycle(commit.id, commits, &mut visited, &mut rec_stack, &mut path)
            {
                return Some(cycle);
            }
        }
    }
    None
}

fn dfs_cycle(
    commit_id: PlannedCommitId,
    commits: &[PlannedCommit],
    visited: &mut HashSet<PlannedCommitId>,
    rec_stack: &mut HashSet<PlannedCommitId>,
    path: &mut Vec<PlannedCommitId>,
) -> Option<Vec<PlannedCommitId>> {
    visited.insert(commit_id);
    rec_stack.insert(commit_id);
    path.push(commit_id);

    if let Some(commit) = commits.iter().find(|c| c.id == commit_id) {
        for dep_id in &commit.depends_on {
            if !visited.contains(dep_id) {
                if let Some(cycle) = dfs_cycle(*dep_id, commits, visited, rec_stack, path) {
                    return Some(cycle);
                }
            } else if rec_stack.contains(dep_id) {
                // Found a cycle - extract the cycle portion from path
                let cycle_start = path.iter().position(|&id| id == *dep_id).unwrap_or(0);
                let mut cycle: Vec<_> = path[cycle_start..].to_vec();
                cycle.push(*dep_id); // Complete the cycle
                return Some(cycle);
            }
        }
    }

    path.pop();
    rec_stack.remove(&commit_id);
    None
}

// =============================================================================
// Deterministic Fix Functions
// =============================================================================

/// Remove duplicate hunk assignments across commits (keeps first occurrence)
pub fn fix_duplicate_hunks(mut commits: Vec<PlannedCommit>) -> Vec<PlannedCommit> {
    let mut seen: HashSet<HunkId> = HashSet::new();

    for commit in &mut commits {
        commit.changes.retain(|change| {
            if let PlannedChange::ExistingHunk(hunk_id) = change {
                seen.insert(*hunk_id)
            } else {
                true // Keep NewHunk changes
            }
        });
    }

    // Remove any commits that ended up with no changes
    commits.retain(|c| !c.changes.is_empty());

    commits
}

/// Assign orphaned hunks to a new catch-all commit
pub fn fix_unassigned_hunks(
    mut commits: Vec<PlannedCommit>,
    hunks: &[Hunk],
) -> Vec<PlannedCommit> {
    let assigned: HashSet<HunkId> = commits
        .iter()
        .flat_map(|c| c.changes.iter())
        .filter_map(|change| {
            if let PlannedChange::ExistingHunk(id) = change {
                Some(*id)
            } else {
                None
            }
        })
        .collect();

    let unassigned: Vec<HunkId> = hunks
        .iter()
        .map(|h| h.id)
        .filter(|id| !assigned.contains(id))
        .collect();

    if !unassigned.is_empty() {
        let next_id = commits.iter().map(|c| c.id.0).max().unwrap_or(0) + 1;
        commits.push(PlannedCommit::from_hunk_ids(
            PlannedCommitId(next_id),
            crate::models::CommitDescription::short_only("Additional changes"),
            unassigned,
        ));
    }

    commits
}

/// Apply all deterministic fixes to a plan
pub fn apply_deterministic_fixes(
    commits: Vec<PlannedCommit>,
    hunks: &[Hunk],
) -> Vec<PlannedCommit> {
    let deduped = fix_duplicate_hunks(commits);
    fix_unassigned_hunks(deduped, hunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CommitDescription, DiffLine};
    use std::path::PathBuf;

    fn make_hunk(id: usize) -> Hunk {
        Hunk {
            id: HunkId(id),
            file_path: PathBuf::from("test.rs"),
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 2,
            lines: vec![
                DiffLine::Context("ctx".into()),
                DiffLine::Added("add".into()),
            ],
            likely_source_commits: vec![],
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        }
    }

    fn make_commit(id: usize, desc: &str, hunk_ids: Vec<usize>) -> PlannedCommit {
        PlannedCommit::from_hunk_ids(
            PlannedCommitId(id),
            CommitDescription::short_only(desc),
            hunk_ids.into_iter().map(HunkId).collect(),
        )
    }

    #[test]
    fn test_valid_plan() {
        let hunks = vec![make_hunk(0), make_hunk(1)];
        let commits = vec![
            make_commit(0, "First commit", vec![0]),
            make_commit(1, "Second commit", vec![1]),
        ];

        let result = validate_plan(&commits, &hunks);
        assert!(result.is_valid());
    }

    #[test]
    fn test_empty_message() {
        let hunks = vec![make_hunk(0)];
        let commits = vec![make_commit(0, "", vec![0])];

        let result = validate_plan(&commits, &hunks);
        assert!(!result.is_valid());
        assert!(matches!(
            &result.issues[0],
            ValidationIssue::EmptyMessage { .. }
        ));
    }

    #[test]
    fn test_unassigned_hunks() {
        let hunks = vec![make_hunk(0), make_hunk(1), make_hunk(2)];
        let commits = vec![make_commit(0, "Only assigns one", vec![0])];

        let result = validate_plan(&commits, &hunks);
        assert!(!result.is_valid());

        let unassigned = result.unassigned_hunks().unwrap();
        assert_eq!(unassigned.len(), 2);
    }

    #[test]
    fn test_duplicate_across_commits() {
        let hunks = vec![make_hunk(0), make_hunk(1)];
        let commits = vec![
            make_commit(0, "First", vec![0, 1]),
            make_commit(1, "Second", vec![1]), // Hunk 1 is duplicate
        ];

        let result = validate_plan(&commits, &hunks);
        assert!(!result.is_valid());

        let dups = result.duplicate_hunks();
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].0, HunkId(1));
    }

    #[test]
    fn test_duplicate_within_commit() {
        let hunks = vec![make_hunk(0)];
        let mut commit = make_commit(0, "Duplicate", vec![0]);
        commit.changes.push(PlannedChange::ExistingHunk(HunkId(0)));

        let result = validate_plan(&[commit], &hunks);
        assert!(!result.is_valid());
        assert!(matches!(
            &result.issues[0],
            ValidationIssue::DuplicateHunkInCommit { .. }
        ));
    }

    #[test]
    fn test_invalid_hunk_reference() {
        let hunks = vec![make_hunk(0)];
        let commits = vec![make_commit(0, "Invalid ref", vec![999])];

        let result = validate_plan(&commits, &hunks);
        assert!(!result.is_valid());
        assert!(matches!(
            &result.issues[0],
            ValidationIssue::InvalidHunk { hunk_id, .. } if hunk_id.0 == 999
        ));
    }

    #[test]
    fn test_cyclic_dependency() {
        let hunks = vec![make_hunk(0), make_hunk(1)];
        let commits = vec![
            PlannedCommit::with_dependencies(
                PlannedCommitId(0),
                CommitDescription::short_only("A"),
                vec![PlannedChange::ExistingHunk(HunkId(0))],
                vec![PlannedCommitId(1)], // A depends on B
            ),
            PlannedCommit::with_dependencies(
                PlannedCommitId(1),
                CommitDescription::short_only("B"),
                vec![PlannedChange::ExistingHunk(HunkId(1))],
                vec![PlannedCommitId(0)], // B depends on A - cycle!
            ),
        ];

        let result = validate_plan(&commits, &hunks);
        assert!(!result.is_valid());
        assert!(result
            .issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::CyclicDependency { .. })));
    }

    #[test]
    fn test_has_fixable_issues() {
        let hunks = vec![make_hunk(0), make_hunk(1)];
        let commits = vec![make_commit(0, "Only one", vec![0])];

        let result = validate_plan(&commits, &hunks);
        assert!(result.has_fixable_issues()); // Unassigned hunk is fixable
    }

    // Fix function tests

    #[test]
    fn test_fix_duplicate_hunks() {
        let commits = vec![
            make_commit(0, "First", vec![0, 1]),
            make_commit(1, "Second", vec![1, 2]), // Hunk 1 is duplicate
        ];

        let fixed = fix_duplicate_hunks(commits);

        assert_eq!(fixed.len(), 2);
        assert_eq!(fixed[0].changes.len(), 2); // Keeps 0, 1
        assert_eq!(fixed[1].changes.len(), 1); // Only keeps 2
    }

    #[test]
    fn test_fix_duplicate_removes_empty_commits() {
        let commits = vec![
            make_commit(0, "First", vec![0, 1]),
            make_commit(1, "Second", vec![0, 1]), // All duplicates
        ];

        let fixed = fix_duplicate_hunks(commits);

        assert_eq!(fixed.len(), 1);
        assert_eq!(fixed[0].description.short, "First");
    }

    #[test]
    fn test_fix_unassigned_hunks() {
        let hunks = vec![make_hunk(0), make_hunk(1), make_hunk(2)];
        let commits = vec![make_commit(0, "Only one", vec![0])];

        let fixed = fix_unassigned_hunks(commits, &hunks);

        assert_eq!(fixed.len(), 2);
        assert_eq!(fixed[1].description.short, "Additional changes");
        assert_eq!(fixed[1].changes.len(), 2); // Hunks 1 and 2
    }

    #[test]
    fn test_apply_deterministic_fixes() {
        let hunks = vec![make_hunk(0), make_hunk(1), make_hunk(2)];
        let commits = vec![
            make_commit(0, "First", vec![0]),
            make_commit(1, "Second", vec![0]), // Duplicate
        ];

        let fixed = apply_deterministic_fixes(commits, &hunks);
        let result = validate_plan(&fixed, &hunks);

        // Should be valid after fixes (no duplicates, no unassigned)
        assert!(result.is_valid(), "Issues: {:?}", result.issues);
    }
}
