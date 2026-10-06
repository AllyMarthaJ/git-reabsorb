//! Native absorb against real repositories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use git_reabsorb::absorb::{absorb, AbsorbError, Leftover};
use git_reabsorb::git::Git;

struct Repo {
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().to_path_buf();
        let repo = Self { path, _dir: dir };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "test@example.com"]);
        repo.git(&["config", "user.name", "Test User"]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.path)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn write(&self, name: &str, content: &str) {
        fs::write(self.path.join(name), content).expect("write");
    }

    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn ops(&self) -> Git {
        Git::with_work_dir(&self.path)
    }

    fn subjects(&self, range: &str) -> Vec<String> {
        self.git(&["log", "--reverse", "--format=%s", range])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn show(&self, revision: &str, file: &str) -> String {
        self.git(&["show", &format!("{revision}:{file}")])
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

/// `main` holds a.txt and b.txt; the branch changes a line of each in its
/// own commit.
fn branch() -> (Repo, String, String, String) {
    let repo = Repo::new();
    repo.write("a.txt", "one\ntwo\nthree\nfour\nfive\n");
    repo.write("b.txt", "x\ny\nz\n");
    let base = repo.commit("base");
    repo.write("a.txt", "one\ntwo\nTHREE\nfour\nfive\n");
    let first = repo.commit("first");
    repo.write("b.txt", "x\nY\nz\n");
    let second = repo.commit("second");
    (repo, base, first, second)
}

#[test]
fn test_changes_fold_into_the_commits_that_last_touched_their_lines() {
    let (repo, base, first, second) = branch();
    // THREE belongs to `first`, Y to `second`, `one` to the base.
    repo.write("a.txt", "one!\ntwo\nTHREE!\nfour\nfive\n");
    repo.write("b.txt", "x\nY!\nz\n");

    let absorption = absorb(
        &repo.ops(),
        &base,
        &Leftover::Commit("leftover".to_string()),
    )
    .unwrap();

    assert_eq!(absorption.absorbed, [(first, 1), (second, 1)]);
    assert_eq!(
        repo.subjects(&format!("{base}..HEAD")),
        ["first", "second", "leftover"]
    );
    assert_eq!(repo.show("HEAD~2", "a.txt"), "one\ntwo\nTHREE!\nfour\nfive");
    assert_eq!(repo.show("HEAD~1", "b.txt"), "x\nY!\nz");
    assert_eq!(repo.show("HEAD", "a.txt"), "one!\ntwo\nTHREE!\nfour\nfive");
    assert_eq!(
        absorption.leftover.as_deref(),
        Some(absorption.head.as_str())
    );
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
}

#[test]
fn test_an_addition_next_to_a_commits_lines_joins_that_commit() {
    let (repo, base, first, _) = branch();
    repo.write("a.txt", "one\ntwo\nTHREE\nthree and a half\nfour\nfive\n");

    let absorption = absorb(&repo.ops(), &base, &Leftover::Keep).unwrap();

    assert_eq!(absorption.absorbed, [(first, 1)]);
    assert_eq!(repo.subjects(&format!("{base}..HEAD")), ["first", "second"]);
    assert_eq!(
        repo.show("HEAD~1", "a.txt"),
        "one\ntwo\nTHREE\nthree and a half\nfour\nfive"
    );
}

#[test]
fn test_kept_leftovers_stay_uncommitted() {
    let (repo, base, first, _) = branch();
    repo.write("a.txt", "one!\ntwo\nTHREE!\nfour\nfive\n");

    let absorption = absorb(&repo.ops(), &base, &Leftover::Keep).unwrap();

    assert_eq!(absorption.absorbed, [(first, 1)]);
    assert_eq!(absorption.leftover, None);
    assert_eq!(repo.subjects(&format!("{base}..HEAD")), ["first", "second"]);
    assert_eq!(repo.git(&["diff", "--name-only"]), "a.txt");
    assert_eq!(
        fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "one!\ntwo\nTHREE!\nfour\nfive\n"
    );
}

#[test]
fn test_a_branch_with_merges_is_refused() {
    let (repo, base, _, _) = branch();
    repo.git(&["checkout", "-q", "-b", "side", &base]);
    repo.write("c.txt", "c\n");
    repo.commit("side");
    repo.git(&["checkout", "-q", "main"]);
    repo.git(&["merge", "-q", "--no-edit", "side"]);
    repo.write("a.txt", "one\ntwo\nTHREE!\nfour\nfive\n");

    let error = absorb(&repo.ops(), &base, &Leftover::Keep).unwrap_err();

    assert!(matches!(error, AbsorbError::Merges(_)), "{error}");
}

#[test]
fn test_a_conflicting_fold_leaves_the_branch_as_it_was() {
    let repo = Repo::new();
    repo.write("a.txt", "one\ntwo\nthree\n");
    let base = repo.commit("base");
    repo.write("a.txt", "one\nTWO\nthree\n");
    repo.commit("first");
    // `second` changes the line right after the one `first` wrote, so a
    // fixup of `first` cannot slide under `second` without a conflict.
    repo.write("a.txt", "one\nTWO\nTHREE\n");
    let second = repo.commit("second");
    repo.write("a.txt", "one\nTWO!\nTHREE\n");
    let before = repo.git(&["diff", "HEAD"]);

    let error = absorb(
        &repo.ops(),
        &base,
        &Leftover::Commit("leftover".to_string()),
    )
    .unwrap_err();

    assert!(matches!(error, AbsorbError::Conflict), "{error}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), second);
    assert_eq!(repo.git(&["diff", "HEAD"]), before);
    assert_eq!(repo.git(&["diff", "--cached"]), "");
}
