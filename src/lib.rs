//! git-reabsorb: Reorganize git commits by unstaging and recommitting
//!
//! This crate provides tools to:
//! - Read commits and their hunks from a git repository
//! - Reorganize hunks into new commits using pluggable strategies
//! - Create new commits with edited descriptions
//!
//! # Architecture
//!
//! The crate is organized around traits that allow for testing and flexibility:
//!
//! - [`git::GitOps`] - Git operations (read commits, reset, commit)
//! - [`reorganize::Reorganizer`] - Strategies for reorganizing hunks
//! - [`editor::Editor`] - Opening the system editor for commit messages
//!
//! # Example
//!
//! ```ignore
//! use git_reabsorb::git::{Git, GitOps};
//! use git_reabsorb::reorganize::{PreserveOriginal, Reorganizer};
//!
//! let git = Git::new();
//! let base = git.find_branch_base()?;
//! let head = git.get_head()?;
//!
//! let commits = git.read_commits(&base, &head)?;
//! let hunks = /* read hunks from commits */;
//!
//! let reorganizer = PreserveOriginal;
//! let planned = reorganizer.reorganize(&commits, &hunks)?;
//! ```

pub mod app;
pub mod assessment;
pub mod cancel;
pub mod cli;
pub mod diff_parser;
pub mod editor;
pub mod features;
pub mod git;
pub mod llm;
pub mod models;
pub mod patch;
pub mod plan_store;
pub mod reorganize;
pub mod utils;

#[cfg(test)]
pub mod test_utils;
