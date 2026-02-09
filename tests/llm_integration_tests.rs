//! Integration tests for LLM-based reorganization
//!
//! These tests require a real LLM (claude CLI) to be available and are ignored by default.
//! Run with: cargo test --test llm_integration_tests -- --ignored
//! Or in CI where claude is available.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use git_reabsorb::git::{Git, GitOps};
use git_reabsorb::llm::ClaudeCliClient;
use git_reabsorb::reorganize::{LlmReorganizer, Reorganizer};

struct TestRepo {
    path: PathBuf,
    git: Git,
}

impl TestRepo {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("git-reabsorb-llm-test-{}", uuid()));
        if path.exists() {
            let _ = fs::remove_dir_all(&path);
        }
        fs::create_dir_all(&path).expect("Failed to create temp dir");

        run_git(&path, &["init", "-b", "main"]);
        run_git(&path, &["config", "user.email", "test@example.com"]);
        run_git(&path, &["config", "user.name", "Test User"]);

        let git = Git::with_work_dir(&path);

        Self { path, git }
    }

    fn write_file(&self, name: &str, content: &str) -> PathBuf {
        let file_path = self.path.join(name);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create parent dirs");
        }
        fs::write(&file_path, content).expect("Failed to write file");
        file_path
    }

    fn stage_all(&self) {
        run_git(&self.path, &["add", "-A"]);
    }

    fn commit(&self, message: &str) -> String {
        run_git(&self.path, &["commit", "-m", message]);
        self.git.get_head().expect("Failed to get HEAD")
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_git(path: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .expect("Failed to run git");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("git {} failed: {}", args.join(" "), stderr);
    }

    String::from_utf8_lossy(&output.stdout).to_string()
}

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    format!("{}-{}", duration.as_secs(), std::process::id())
}

fn claude_available() -> bool {
    Command::new("claude")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Test that the LLM reorganizer can process a simple commit range
#[test]
#[ignore] // Requires real claude CLI
fn test_llm_reorganizer_simple_commits() {
    if !claude_available() {
        eprintln!("Skipping test: claude CLI not available");
        return;
    }

    let repo = TestRepo::new();

    // Create initial commit
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create a commit with changes to two files that could be logically separated
    repo.write_file("src/main.rs", "fn main() {\n    println!(\"Hello\");\n}\n");
    repo.write_file(
        "Cargo.toml",
        "[package]\nname = \"test\"\nversion = \"0.1.0\"\n",
    );
    repo.stage_all();
    let _head = repo.commit("Add main and cargo config");

    // Get commits and hunks
    let commits = repo
        .git
        .read_commits(&base, "HEAD")
        .expect("Failed to read commits");
    let diff = repo
        .git
        .diff_trees(&base, "HEAD")
        .expect("Failed to get diff");

    let parsed = git_reabsorb::patch::parse(&diff, &[], 0).expect("Failed to parse diff");

    assert!(!parsed.hunks.is_empty(), "Should have hunks to reorganize");

    // Create LLM reorganizer with real claude
    let client = Box::new(ClaudeCliClient::new());
    let reorganizer = LlmReorganizer::new(client);

    // Run reorganization
    let result = reorganizer.plan(&commits, &parsed.hunks);
    assert!(
        result.is_ok(),
        "LLM reorganization failed: {:?}",
        result.err()
    );

    let planned_commits = result.unwrap();
    assert!(
        !planned_commits.is_empty(),
        "Should produce at least one commit"
    );

    // Verify all hunks are assigned
    let total_changes: usize = planned_commits.iter().map(|c| c.changes.len()).sum();
    assert!(
        total_changes >= parsed.hunks.len(),
        "All hunks should be assigned (got {} changes for {} hunks)",
        total_changes,
        parsed.hunks.len()
    );

    println!("LLM produced {} commits:", planned_commits.len());
    for (i, commit) in planned_commits.iter().enumerate() {
        println!(
            "  {}. \"{}\" ({} changes)",
            i + 1,
            commit.description.short,
            commit.changes.len()
        );
    }
}

/// Test that the LLM can handle a larger set of changes
#[test]
#[ignore] // Requires real claude CLI
fn test_llm_reorganizer_multiple_commits() {
    if !claude_available() {
        eprintln!("Skipping test: claude CLI not available");
        return;
    }

    let repo = TestRepo::new();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let base = repo.commit("Initial");

    // Create multiple commits with related and unrelated changes
    repo.write_file(
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    );
    repo.stage_all();
    repo.commit("Add add function");

    repo.write_file(
        "tests/test.rs",
        "#[test]\nfn test_add() { assert_eq!(add(1, 2), 3); }\n",
    );
    repo.stage_all();
    repo.commit("Add test for add");

    repo.write_file("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }\npub fn sub(a: i32, b: i32) -> i32 { a - b }\n");
    repo.stage_all();
    repo.commit("Add sub function");

    // Get commits and hunks
    let commits = repo
        .git
        .read_commits(&base, "HEAD")
        .expect("Failed to read commits");
    let diff = repo
        .git
        .diff_trees(&base, "HEAD")
        .expect("Failed to get diff");

    let parsed = git_reabsorb::patch::parse(&diff, &[], 0).expect("Failed to parse diff");

    // Create LLM reorganizer
    let client = Box::new(ClaudeCliClient::new());
    let reorganizer = LlmReorganizer::new(client);

    let result = reorganizer.plan(&commits, &parsed.hunks);
    assert!(
        result.is_ok(),
        "LLM reorganization failed: {:?}",
        result.err()
    );

    let planned_commits = result.unwrap();
    println!(
        "LLM produced {} commits from {} original",
        planned_commits.len(),
        commits.len()
    );

    for commit in &planned_commits {
        println!(
            "  - {} ({} changes)",
            commit.description.short,
            commit.changes.len()
        );
    }
}
