//! Integration tests for reorganizers using real git repositories

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use git_scramble::git::{Git, GitOps};
use git_scramble::models::Hunk;
use git_scramble::reorganize::{GroupByFile, PreserveOriginal, Reorganizer, Squash};

/// A temporary git repository for testing
struct TestRepo {
    path: PathBuf,
    git: Git,
}

impl TestRepo {
    /// Create a new temporary git repository
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("git-scramble-test-{}", uuid()));
        fs::create_dir_all(&path).expect("Failed to create temp dir");

        // Initialize git repo
        run_git(&path, &["init"]);
        run_git(&path, &["config", "user.email", "test@example.com"]);
        run_git(&path, &["config", "user.name", "Test User"]);

        let git = Git::with_work_dir(&path);

        Self { path, git }
    }

    /// Write a file and return its path
    fn write_file(&self, name: &str, content: &str) -> PathBuf {
        let file_path = self.path.join(name);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create parent dirs");
        }
        fs::write(&file_path, content).expect("Failed to write file");
        file_path
    }

    /// Stage all changes
    fn stage_all(&self) {
        run_git(&self.path, &["add", "-A"]);
    }

    /// Create a commit with the given message
    fn commit(&self, message: &str) -> String {
        run_git(&self.path, &["commit", "-m", message]);
        self.git.get_head().expect("Failed to get HEAD")
    }

    /// Read commits in a range
    fn read_commits(
        &self,
        base: &str,
        head: &str,
    ) -> Vec<git_scramble::models::SourceCommit> {
        self.git
            .read_commits(base, head)
            .expect("Failed to read commits")
    }

    /// Read all hunks from commits
    fn read_hunks(&self, commits: &[git_scramble::models::SourceCommit]) -> Vec<Hunk> {
        let mut all_hunks = Vec::new();
        let mut hunk_id = 0;
        for commit in commits {
            let hunks = self
                .git
                .read_hunks(&commit.sha, hunk_id)
                .expect("Failed to read hunks");
            hunk_id += hunks.len();
            all_hunks.extend(hunks);
        }
        all_hunks
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        // Clean up temp directory
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Run a git command in the given directory
fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("Failed to run git");

    if !output.status.success() {
        panic!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Generate a simple unique ID
fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    format!("{}-{}", duration.as_secs(), duration.subsec_nanos())
}

// ============================================================================
// PreserveOriginal Tests
// ============================================================================

#[test]
fn test_preserve_original_single_commit() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create one commit to scramble
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let _head = repo.commit("Add main.rs");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    let reorganizer = PreserveOriginal;
    let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

    // Should have exactly 1 planned commit
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].description.short, "Add main.rs");
    assert_eq!(planned[0].hunk_ids.len(), 1);
}

#[test]
fn test_preserve_original_multiple_commits() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create first commit
    repo.write_file("src/main.rs", "fn main() {\n    println!(\"Hello\");\n}\n");
    repo.stage_all();
    repo.commit("Add main.rs");

    // Create second commit
    repo.write_file("src/lib.rs", "pub fn greet() {}\n");
    repo.stage_all();
    repo.commit("Add lib.rs");

    // Create third commit - modifies existing file
    repo.write_file(
        "src/main.rs",
        "fn main() {\n    println!(\"Hello\");\n    println!(\"World\");\n}\n",
    );
    repo.stage_all();
    repo.commit("Update main.rs");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    assert_eq!(commits.len(), 3);

    let reorganizer = PreserveOriginal;
    let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

    // Should preserve all 3 commits in order
    assert_eq!(planned.len(), 3);
    assert_eq!(planned[0].description.short, "Add main.rs");
    assert_eq!(planned[1].description.short, "Add lib.rs");
    assert_eq!(planned[2].description.short, "Update main.rs");
}

#[test]
fn test_preserve_original_commit_with_multiple_files() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create commit with multiple files
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.write_file("src/lib.rs", "pub mod utils;\n");
    repo.write_file("src/utils.rs", "pub fn helper() {}\n");
    repo.stage_all();
    repo.commit("Add source files");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    assert_eq!(hunks.len(), 3); // 3 files = 3 hunks

    let reorganizer = PreserveOriginal;
    let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

    // Should have 1 commit with all 3 hunks
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].hunk_ids.len(), 3);
}

// ============================================================================
// GroupByFile Tests
// ============================================================================

#[test]
fn test_group_by_file_single_file() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create commit
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    repo.commit("Add main.rs");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    let reorganizer = GroupByFile;
    let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

    // Should have 1 commit for 1 file
    assert_eq!(planned.len(), 1);
    assert!(planned[0].description.short.contains("main.rs"));
}

#[test]
fn test_group_by_file_multiple_files_single_commit() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create commit with multiple files
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.write_file("src/lib.rs", "pub mod utils;\n");
    repo.write_file("tests/test.rs", "#[test] fn test() {}\n");
    repo.stage_all();
    repo.commit("Add files");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    let reorganizer = GroupByFile;
    let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

    // Should have 3 commits, one per file
    assert_eq!(planned.len(), 3);

    // Each commit should have exactly 1 hunk
    for commit in &planned {
        assert_eq!(commit.hunk_ids.len(), 1);
    }
}

#[test]
fn test_group_by_file_same_file_multiple_commits() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // First commit - create file
    repo.write_file("src/main.rs", "fn main() {\n}\n");
    repo.stage_all();
    repo.commit("Add main.rs");

    // Second commit - modify same file
    repo.write_file("src/main.rs", "fn main() {\n    println!(\"Hello\");\n}\n");
    repo.stage_all();
    repo.commit("Add print statement");

    // Third commit - modify same file again
    repo.write_file(
        "src/main.rs",
        "fn main() {\n    println!(\"Hello\");\n    println!(\"World\");\n}\n",
    );
    repo.stage_all();
    repo.commit("Add another print");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    assert_eq!(commits.len(), 3);
    assert_eq!(hunks.len(), 3); // Each commit has 1 hunk

    let reorganizer = GroupByFile;
    let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

    // Should have 1 commit with all hunks for main.rs
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].hunk_ids.len(), 3);
    assert!(planned[0].description.short.contains("main.rs"));
}

#[test]
fn test_group_by_file_interleaved_changes() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Commit 1: main.rs
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    repo.commit("Add main");

    // Commit 2: lib.rs
    repo.write_file("src/lib.rs", "pub fn lib() {}\n");
    repo.stage_all();
    repo.commit("Add lib");

    // Commit 3: main.rs again
    repo.write_file("src/main.rs", "fn main() {\n    lib();\n}\n");
    repo.stage_all();
    repo.commit("Use lib in main");

    // Commit 4: lib.rs again
    repo.write_file("src/lib.rs", "pub fn lib() {\n    println!(\"lib\");\n}\n");
    repo.stage_all();
    repo.commit("Implement lib");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    assert_eq!(commits.len(), 4);

    let reorganizer = GroupByFile;
    let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

    // Should have 2 commits: one for main.rs, one for lib.rs
    assert_eq!(planned.len(), 2);

    // Find main.rs commit
    let main_commit = planned
        .iter()
        .find(|c| c.description.short.contains("main.rs"))
        .expect("Should have main.rs commit");
    assert_eq!(main_commit.hunk_ids.len(), 2);

    // Find lib.rs commit
    let lib_commit = planned
        .iter()
        .find(|c| c.description.short.contains("lib.rs"))
        .expect("Should have lib.rs commit");
    assert_eq!(lib_commit.hunk_ids.len(), 2);
}

// ============================================================================
// Squash Tests
// ============================================================================

#[test]
fn test_squash_single_commit() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create one commit
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    repo.commit("Add main.rs");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    let reorganizer = Squash;
    let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

    // Should have 1 commit
    assert_eq!(planned.len(), 1);
    // Single commit should preserve original message
    assert_eq!(planned[0].description.short, "Add main.rs");
}

#[test]
fn test_squash_multiple_commits() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create multiple commits
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    repo.commit("Add main.rs");

    repo.write_file("src/lib.rs", "pub fn lib() {}\n");
    repo.stage_all();
    repo.commit("Add lib.rs");

    repo.write_file("tests/test.rs", "#[test] fn test() {}\n");
    repo.stage_all();
    repo.commit("Add tests");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    assert_eq!(commits.len(), 3);
    assert_eq!(hunks.len(), 3);

    let reorganizer = Squash;
    let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

    // Should have exactly 1 commit with all hunks
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].hunk_ids.len(), 3);
    assert!(planned[0].description.short.contains("Squashed"));
    assert!(planned[0].description.long.contains("Add main.rs"));
    assert!(planned[0].description.long.contains("Add lib.rs"));
    assert!(planned[0].description.long.contains("Add tests"));
}

#[test]
fn test_squash_many_hunks() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test Project\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create commit with multiple files (multiple hunks)
    repo.write_file("src/a.rs", "pub fn a() {}\n");
    repo.write_file("src/b.rs", "pub fn b() {}\n");
    repo.write_file("src/c.rs", "pub fn c() {}\n");
    repo.stage_all();
    repo.commit("Add a, b, c");

    // Modify all files
    repo.write_file("src/a.rs", "pub fn a() { println!(\"a\"); }\n");
    repo.write_file("src/b.rs", "pub fn b() { println!(\"b\"); }\n");
    repo.write_file("src/c.rs", "pub fn c() { println!(\"c\"); }\n");
    repo.stage_all();
    repo.commit("Implement a, b, c");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    assert_eq!(commits.len(), 2);
    assert_eq!(hunks.len(), 6); // 3 files * 2 commits

    let reorganizer = Squash;
    let planned = reorganizer.reorganize(&commits, &hunks).unwrap();

    // Should squash everything into 1 commit
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].hunk_ids.len(), 6);
}

// ============================================================================
// Edge Case Tests
// ============================================================================

#[test]
fn test_file_with_multiple_hunks_in_single_commit() {
    let repo = TestRepo::new();

    // Create initial commit with a file
    repo.write_file(
        "src/main.rs",
        "fn main() {\n    // start\n}\n\nfn helper() {\n    // helper\n}\n",
    );
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Modify file in multiple places (creates multiple hunks)
    repo.write_file(
        "src/main.rs",
        "fn main() {\n    println!(\"start\");\n}\n\nfn helper() {\n    println!(\"helper\");\n}\n",
    );
    repo.stage_all();
    repo.commit("Add print statements");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    // Might have 1 or 2 hunks depending on git's diff algorithm
    assert!(!hunks.is_empty());

    // All reorganizers should handle this
    let preserve = PreserveOriginal;
    let by_file = GroupByFile;
    let squash = Squash;

    let preserve_planned = preserve.reorganize(&commits, &hunks).unwrap();
    let by_file_planned = by_file.reorganize(&commits, &hunks).unwrap();
    let squash_planned = squash.reorganize(&commits, &hunks).unwrap();

    assert_eq!(preserve_planned.len(), 1);
    assert_eq!(by_file_planned.len(), 1); // All hunks in same file
    assert_eq!(squash_planned.len(), 1);
}

#[test]
fn test_deleted_file() {
    let repo = TestRepo::new();

    // Create initial commit with a file
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.write_file("src/to_delete.rs", "// This will be deleted\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Delete the file
    fs::remove_file(repo.path.join("src/to_delete.rs")).unwrap();
    repo.stage_all();
    repo.commit("Delete to_delete.rs");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    assert_eq!(commits.len(), 1);
    assert_eq!(hunks.len(), 1);

    // All reorganizers should handle deletions
    let preserve = PreserveOriginal;
    let planned = preserve.reorganize(&commits, &hunks).unwrap();

    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].hunk_ids.len(), 1);
}

#[test]
fn test_renamed_file() {
    let repo = TestRepo::new();

    // Create initial commit with a file
    repo.write_file("src/old_name.rs", "fn old() {}\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Rename the file
    run_git(&repo.path, &["mv", "src/old_name.rs", "src/new_name.rs"]);
    repo.commit("Rename file");

    // Read and reorganize
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    // Git might show this as a rename (no hunks) or as delete+add
    // Either way, reorganizers should handle it gracefully
    let preserve = PreserveOriginal;

    if !hunks.is_empty() {
        let planned = preserve.reorganize(&commits, &hunks).unwrap();
        assert!(!planned.is_empty());
    }
    // If no hunks (pure rename), that's also valid behavior
}

#[test]
fn test_empty_file_creation() {
    let repo = TestRepo::new();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create an empty file
    repo.write_file("src/.gitkeep", "");
    repo.stage_all();
    repo.commit("Add empty file");

    // Read and reorganize - empty files might not create hunks
    let commits = repo.read_commits(&base, "HEAD");
    let hunks = repo.read_hunks(&commits);

    // This is valid regardless of whether hunks were created
    if !hunks.is_empty() {
        let preserve = PreserveOriginal;
        let planned = preserve.reorganize(&commits, &hunks).unwrap();
        assert!(!planned.is_empty());
    }
}
