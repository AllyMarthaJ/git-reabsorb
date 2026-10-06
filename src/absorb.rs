//! Absorb uncommitted changes into the branch commits that last touched the
//! lines they change, as `git absorb` does, without the external binary.
//!
//! Each hunk of the working tree's diff against `HEAD` is blamed over
//! `base..HEAD`. A hunk whose changed lines were all last touched by one
//! commit on the branch becomes a fixup of that commit; an autosquash rebase
//! folds the fixups in. Hunks no single branch commit introduced are left for
//! the caller: committed on top, or kept uncommitted.

use std::collections::BTreeMap;

use crate::git::{GitError, GitOps};
use crate::models::{ChangeType, Hunk};
use crate::patch::PatchContext;

#[derive(Debug, thiserror::Error)]
pub enum AbsorbError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Parse(#[from] crate::patch::ParseError),
    #[error("{0} contains merge commits, which rewriting the branch would flatten")]
    Merges(String),
    #[error("folding the fixups into their commits conflicted; nothing was absorbed")]
    Conflict,
}

/// What happens to the changes no branch commit introduced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Leftover {
    /// Commit them on top of the rewritten branch with this message.
    Commit(String),
    /// Leave them uncommitted in the working tree.
    Keep,
}

/// What an absorb did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Absorption {
    /// The branch's tip afterwards.
    pub head: String,
    /// Each original commit that took changes, oldest first, with the number
    /// of hunks it took.
    pub absorbed: Vec<(String, usize)>,
    /// The commit holding the changes no branch commit introduced, when they
    /// were committed and there were any.
    pub leftover: Option<String>,
}

/// Absorbs the working tree's changes into the commits of `base..HEAD`.
///
/// On a conflict the branch is left at its original `HEAD` with every change
/// uncommitted, as it was before.
pub fn absorb(
    git: &dyn GitOps,
    base: &str,
    leftover: &Leftover,
) -> Result<Absorption, AbsorbError> {
    let head = git.get_head()?;
    let range = format!("{base}..{head}");
    if !git
        .run_git_output(&["rev-list", "--merges", &range])?
        .trim()
        .is_empty()
    {
        return Err(AbsorbError::Merges(range));
    }
    let branch: Vec<String> = git
        .run_git_output(&["rev-list", "--reverse", &range])?
        .lines()
        .map(str::to_string)
        .collect();
    // No context lines: nearby changes stay separate hunks, each judged on
    // exactly the lines it changes.
    let diff = git.run_git_output(&["diff", &head, "-U0", "--no-color", "--no-renames"])?;
    let patch = crate::patch::parse(&diff, &[], 0)?;

    let mut targets: BTreeMap<usize, Vec<&Hunk>> = BTreeMap::new();
    for hunk in &patch.hunks {
        let modified = patch
            .file_changes
            .iter()
            .find(|change| change.file_path == hunk.file_path)
            .is_none_or(|change| change.change_type == ChangeType::Modified && !change.is_binary);
        if !modified {
            continue;
        }
        if let Some(index) = target(git, &head, &range, &branch, hunk)? {
            targets.entry(index).or_default().push(hunk);
        }
    }

    let context = PatchContext::new(&patch.file_changes);
    let mut applied: Vec<&Hunk> = Vec::new();
    let mut absorbed = Vec::new();
    for (index, hunks) in &targets {
        // Every fixup is the original HEAD plus all hunks so far, so hunks
        // always apply against the lines they were cut from.
        applied.extend(hunks);
        git.run_git_output(&["read-tree", &head])?;
        git.apply_hunks_to_index(&applied, &context)?;
        git.commit(&format!("fixup! {}", branch[*index]), true)?;
        absorbed.push((branch[*index].clone(), hunks.len()));
    }

    let mut leftover_commit = None;
    if let Leftover::Commit(message) = leftover {
        git.stage_all()?;
        let staged = git.run_git_output(&["diff", "--cached", "--name-only"])?;
        if !staged.trim().is_empty() {
            leftover_commit = Some(git.commit(message, true)?);
        }
    } else {
        // The fixups were committed from the index alone; the rest of the
        // working tree stays as the caller left it.
        git.run_git_output(&["read-tree", "HEAD"])?;
    }

    if !absorbed.is_empty() {
        let tip = git.get_head()?;
        let mut rebase = vec![
            "-c",
            "sequence.editor=true",
            "rebase",
            "--interactive",
            "--autosquash",
        ];
        if *leftover == Leftover::Keep {
            rebase.push("--autostash");
        }
        rebase.push(base);
        if git.run_git_output(&rebase).is_err() {
            let _ = git.run_git_output(&["rebase", "--abort"]);
            // `tip` holds every change; put the branch back at `head` with
            // them uncommitted.
            git.run_git_output(&["reset", "--soft", &tip])?;
            git.run_git_output(&["reset", "--soft", &head])?;
            git.run_git_output(&["reset"])?;
            return Err(AbsorbError::Conflict);
        }
        if let Some(commit) = &mut leftover_commit {
            *commit = git.get_head()?;
        }
    }

    Ok(Absorption {
        head: git.get_head()?,
        absorbed,
        leftover: leftover_commit,
    })
}

/// The index in `branch` of the one commit that last touched every line
/// `hunk` changes, if there is one. A hunk that only adds lines goes to the
/// branch commit that wrote the lines either side of it, when only one did.
fn target(
    git: &dyn GitOps,
    head: &str,
    range: &str,
    branch: &[String],
    hunk: &Hunk,
) -> Result<Option<usize>, AbsorbError> {
    let path = hunk.file_path.to_string_lossy();
    let lines: Vec<u32> = if hunk.old_count > 0 {
        (hunk.old_start..hunk.old_start + hunk.old_count).collect()
    } else {
        // A pure addition's `old_start` is the line it follows (0 at the top).
        let length = git
            .run_git_output(&["show", &format!("{head}:{path}")])?
            .lines()
            .count();
        [hunk.old_start, hunk.old_start + 1]
            .into_iter()
            .filter(|line| *line >= 1 && *line as usize <= length)
            .collect()
    };
    if lines.is_empty() {
        return Ok(None);
    }
    let ranges: Vec<String> = lines.iter().map(|line| format!("{line},{line}")).collect();
    let mut args = vec!["blame", "-l", "-s"];
    for range in &ranges {
        args.extend(["-L", range]);
    }
    args.extend([range, "--", &path]);
    let blame = git.run_git_output(&args)?;
    let mut owner: Option<usize> = None;
    for entry in blame.lines() {
        let sha = entry.split_whitespace().next().unwrap_or_default();
        // `^` marks a line older than the range: no branch commit owns it.
        let Some(index) = branch.iter().position(|commit| commit == sha) else {
            // A changed line the branch did not write cannot be absorbed; an
            // added line beside one is judged by its other neighbour.
            if hunk.old_count > 0 {
                return Ok(None);
            }
            continue;
        };
        match owner {
            Some(existing) if existing != index => return Ok(None),
            _ => owner = Some(index),
        }
    }
    Ok(owner)
}
