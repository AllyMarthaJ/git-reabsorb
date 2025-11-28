//! Integration tests for reorganizers using real git repositories

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use git_reabsorb::git::{Git, GitOps};
use git_reabsorb::models::Hunk;
use git_reabsorb::reorganize::{GroupByFile, HierarchicalConfig, HierarchicalReorganizer, PreserveOriginal, Reorganizer, Squash};

struct TestRepo {
    path: PathBuf,
    git: Git,
}

impl TestRepo {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("git-reabsorb-test-{}", uuid()));
        if path.exists() {
            let _ = fs::remove_dir_all(&path);
        }
        fs::create_dir_all(&path).expect("Failed to create temp dir");

        // Initialize git repo with main as default branch
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

    fn read_commits(&self, base: &str, head: &str) -> Vec<git_reabsorb::models::SourceCommit> {
        self.git
            .read_commits(base, head)
            .expect("Failed to read commits")
    }

    fn read_hunks(&self, commits: &[git_reabsorb::models::SourceCommit]) -> Vec<Hunk> {
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
        let _ = fs::remove_dir_all(&self.path);
    }
}

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

fn uuid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    let suffix = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{}-{}-{}",
        duration.as_secs(),
        duration.subsec_nanos(),
        suffix
    )
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

    // Create one commit to reabsorb
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
    assert_eq!(planned[0].changes.len(), 1);
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
    assert_eq!(planned[0].changes.len(), 3);
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
        assert_eq!(commit.changes.len(), 1);
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
    assert_eq!(planned[0].changes.len(), 3);
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
    assert_eq!(main_commit.changes.len(), 2);

    // Find lib.rs commit
    let lib_commit = planned
        .iter()
        .find(|c| c.description.short.contains("lib.rs"))
        .expect("Should have lib.rs commit");
    assert_eq!(lib_commit.changes.len(), 2);
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
    assert_eq!(planned[0].changes.len(), 3);
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
    assert_eq!(planned[0].changes.len(), 6);
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
    assert_eq!(planned[0].changes.len(), 1);
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

// ============================================================================
// Pre-Reabsorb State Tests
// ============================================================================

#[test]
fn test_save_and_get_pre_reabsorb_head() {
    let repo = TestRepo::new();
    let ref_name = test_pre_reabsorb_ref();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let initial_head = repo.commit("Initial commit");

    // Initially, no pre-reabsorb state should exist
    assert!(!repo.git.has_pre_reabsorb_head(&ref_name));

    // Save the pre-reabsorb state
    repo.git.save_pre_reabsorb_head(&ref_name).unwrap();

    // Now it should exist
    assert!(repo.git.has_pre_reabsorb_head(&ref_name));

    // And it should match the current HEAD
    let saved = repo.git.get_pre_reabsorb_head(&ref_name).unwrap();
    assert_eq!(saved, initial_head);
}

#[test]
fn test_clear_pre_reabsorb_head() {
    let repo = TestRepo::new();
    let ref_name = test_pre_reabsorb_ref();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    repo.commit("Initial commit");

    // Save and verify
    repo.git.save_pre_reabsorb_head(&ref_name).unwrap();
    assert!(repo.git.has_pre_reabsorb_head(&ref_name));

    // Clear it
    repo.git.clear_pre_reabsorb_head(&ref_name).unwrap();

    // Should no longer exist
    assert!(!repo.git.has_pre_reabsorb_head(&ref_name));
}

#[test]
fn test_clear_nonexistent_pre_reabsorb_head() {
    let repo = TestRepo::new();
    let ref_name = test_pre_reabsorb_ref();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    repo.commit("Initial commit");

    // Clearing when none exists should not error
    assert!(!repo.git.has_pre_reabsorb_head(&ref_name));
    repo.git.clear_pre_reabsorb_head(&ref_name).unwrap();
    assert!(!repo.git.has_pre_reabsorb_head(&ref_name));
}

#[test]
fn test_get_pre_reabsorb_head_when_none_exists() {
    let repo = TestRepo::new();
    let ref_name = test_pre_reabsorb_ref();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    repo.commit("Initial commit");

    // Should return error when no pre-reabsorb state exists
    assert!(!repo.git.has_pre_reabsorb_head(&ref_name));
    let result = repo.git.get_pre_reabsorb_head(&ref_name);
    assert!(result.is_err());
}

#[test]
fn test_pre_reabsorb_head_survives_new_commits() {
    let repo = TestRepo::new();
    let ref_name = test_pre_reabsorb_ref();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let initial_head = repo.commit("Initial commit");

    // Save pre-reabsorb state
    repo.git.save_pre_reabsorb_head(&ref_name).unwrap();

    // Create more commits
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    repo.commit("Add main.rs");

    repo.write_file("src/lib.rs", "pub fn lib() {}\n");
    repo.stage_all();
    repo.commit("Add lib.rs");

    // Pre-reabsorb state should still point to the original HEAD
    let saved = repo.git.get_pre_reabsorb_head(&ref_name).unwrap();
    assert_eq!(saved, initial_head);
}

#[test]
fn test_reset_to_pre_reabsorb_head() {
    let repo = TestRepo::new();
    let ref_name = test_pre_reabsorb_ref();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let initial_head = repo.commit("Initial commit");

    // Save pre-reabsorb state
    repo.git.save_pre_reabsorb_head(&ref_name).unwrap();

    // Create more commits
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    repo.commit("Add main.rs");

    repo.write_file("src/lib.rs", "pub fn lib() {}\n");
    repo.stage_all();
    let final_head = repo.commit("Add lib.rs");

    // Verify we're at the new HEAD
    assert_eq!(repo.git.get_head().unwrap(), final_head);

    // Reset to pre-reabsorb state
    let pre_reabsorb = repo.git.get_pre_reabsorb_head(&ref_name).unwrap();
    repo.git.reset_hard(&pre_reabsorb).unwrap();

    // Verify we're back at the original HEAD
    assert_eq!(repo.git.get_head().unwrap(), initial_head);

    // Clean up the ref
    repo.git.clear_pre_reabsorb_head(&ref_name).unwrap();
}

#[test]
fn test_overwrite_pre_reabsorb_head() {
    let repo = TestRepo::new();
    let ref_name = test_pre_reabsorb_ref();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let first_head = repo.commit("Initial commit");

    // Save pre-reabsorb state
    repo.git.save_pre_reabsorb_head(&ref_name).unwrap();
    assert_eq!(
        repo.git.get_pre_reabsorb_head(&ref_name).unwrap(),
        first_head
    );

    // Create another commit
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let second_head = repo.commit("Add main.rs");

    // Overwrite pre-reabsorb state
    repo.git.save_pre_reabsorb_head(&ref_name).unwrap();

    // Should now point to the second HEAD
    assert_eq!(
        repo.git.get_pre_reabsorb_head(&ref_name).unwrap(),
        second_head
    );
}

#[test]
fn test_diff_trees_matches_identical_commits() {
    let repo = TestRepo::new();

    repo.write_file("file.txt", "hello\n");
    repo.stage_all();
    let head = repo.commit("init");

    let diff = repo.git.diff_trees(&head, &head).unwrap();
    assert!(diff.trim().is_empty());
}

#[test]
fn test_diff_trees_detects_changes() {
    let repo = TestRepo::new();

    repo.write_file("file.txt", "hello\n");
    repo.stage_all();
    let first = repo.commit("init");

    repo.write_file("file.txt", "hello world\n");
    repo.stage_all();
    let second = repo.commit("update");

    let diff = repo.git.diff_trees(&first, &second).unwrap();
    assert!(diff.contains("diff --git"));
}

// ============================================================================
// Branch Base Tests
// ============================================================================

#[test]
fn test_find_merge_base_with_branch() {
    let repo = TestRepo::new();

    // Create initial commit on main
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let main_head = repo.commit("Initial commit");

    // Create a branch
    run_git(&repo.path, &["checkout", "-b", "feature"]);

    // Add commits on feature branch
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    repo.commit("Add main.rs");

    repo.write_file("src/lib.rs", "pub fn lib() {}\n");
    repo.stage_all();
    repo.commit("Add lib.rs");

    // Find merge-base with main should return the initial commit
    let merge_base = repo.git.find_merge_base("main").unwrap();
    assert_eq!(merge_base, main_head);
}

#[test]
fn test_find_merge_base_with_diverged_branches() {
    let repo = TestRepo::new();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let initial_commit = repo.commit("Initial commit");

    // Create a branch from here
    run_git(&repo.path, &["checkout", "-b", "feature"]);

    // Add commit on feature
    repo.write_file("src/feature.rs", "fn feature() {}\n");
    repo.stage_all();
    repo.commit("Add feature");

    // Go back to main and add a commit there too
    run_git(&repo.path, &["checkout", "main"]);
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    repo.commit("Add main on main branch");

    // Go back to feature
    run_git(&repo.path, &["checkout", "feature"]);

    // Merge base should still be the initial commit
    let merge_base = repo.git.find_merge_base("main").unwrap();
    assert_eq!(merge_base, initial_commit);
}

// ============================================================================
// New File Detection Tests
// ============================================================================

#[test]
fn test_get_new_files_in_commit_detects_added_files() {
    let repo = TestRepo::new();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let _base = repo.commit("Initial commit");

    // Create a commit that adds new files
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.write_file("src/lib.rs", "pub fn lib() {}\n");
    repo.stage_all();
    let commit_sha = repo.commit("Add source files");

    // Check that get_new_files_in_commit finds both files
    let new_files = repo.git.get_new_files_in_commit(&commit_sha).unwrap();
    assert_eq!(new_files.len(), 2);
    assert!(new_files.contains(&"src/main.rs".to_string()));
    assert!(new_files.contains(&"src/lib.rs".to_string()));
}

#[test]
fn test_get_new_files_in_commit_ignores_modified_files() {
    let repo = TestRepo::new();

    // Create initial commit with a file
    repo.write_file("README.md", "# Test\n");
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let _base = repo.commit("Initial commit");

    // Modify existing file and add a new one
    repo.write_file("src/main.rs", "fn main() { println!(\"hello\"); }\n");
    repo.write_file("src/lib.rs", "pub fn lib() {}\n");
    repo.stage_all();
    let commit_sha = repo.commit("Modify main.rs and add lib.rs");

    // Should only detect lib.rs as new, not main.rs
    let new_files = repo.git.get_new_files_in_commit(&commit_sha).unwrap();
    assert_eq!(new_files.len(), 1);
    assert!(new_files.contains(&"src/lib.rs".to_string()));
    assert!(!new_files.contains(&"src/main.rs".to_string()));
}

#[test]
fn test_get_new_files_in_commit_empty_when_only_modifications() {
    let repo = TestRepo::new();

    // Create initial commit with files
    repo.write_file("README.md", "# Test\n");
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let _base = repo.commit("Initial commit");

    // Only modify existing files
    repo.write_file("README.md", "# Updated Test\n");
    repo.write_file("src/main.rs", "fn main() { println!(\"hello\"); }\n");
    repo.stage_all();
    let commit_sha = repo.commit("Update files");

    // Should be empty - no new files
    let new_files = repo.git.get_new_files_in_commit(&commit_sha).unwrap();
    assert!(new_files.is_empty());
}

#[test]
fn test_get_new_files_in_nested_directories() {
    let repo = TestRepo::new();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let _base = repo.commit("Initial commit");

    // Add files in deeply nested directories
    repo.write_file(
        "src/components/ui/Button.tsx",
        "export const Button = () => {};\n",
    );
    repo.write_file(
        "src/components/ui/Input.tsx",
        "export const Input = () => {};\n",
    );
    repo.write_file(
        "src/utils/helpers/string.ts",
        "export const trim = (s: string) => s.trim();\n",
    );
    repo.stage_all();
    let commit_sha = repo.commit("Add nested files");

    let new_files = repo.git.get_new_files_in_commit(&commit_sha).unwrap();
    assert_eq!(new_files.len(), 3);
    assert!(new_files.contains(&"src/components/ui/Button.tsx".to_string()));
    assert!(new_files.contains(&"src/components/ui/Input.tsx".to_string()));
    assert!(new_files.contains(&"src/utils/helpers/string.ts".to_string()));
}

// ============================================================================
// Working Tree Diff Tests
// ============================================================================

#[test]
fn test_working_tree_diff_after_reset_shows_modified_files() {
    let repo = TestRepo::new();

    // Create initial commit with files (so they're tracked)
    repo.write_file("README.md", "# Test\n");
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Modify the tracked files
    repo.write_file("README.md", "# Updated Test\n\nMore content.\n");
    repo.write_file("src/main.rs", "fn main() {\n    println!(\"hello\");\n}\n");
    repo.stage_all();
    repo.commit("Update files");

    // Reset to base (mixed reset keeps changes in working tree)
    repo.git.reset_to(&base).unwrap();

    // Working tree diff should show modifications to TRACKED files
    let diff = repo.git.get_working_tree_diff().unwrap();
    assert!(diff.contains("README.md"), "Should show modified README.md");
    assert!(diff.contains("src/main.rs"), "Should show modified main.rs");
    assert!(
        diff.contains("println!(\"hello\")"),
        "Should show the new content"
    );
}

#[test]
fn test_working_tree_diff_does_not_show_new_untracked_files() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Add NEW files (not tracked in base)
    repo.write_file("src/new_file.rs", "fn new() {}\n");
    repo.stage_all();
    repo.commit("Add new file");

    // Reset to base
    repo.git.reset_to(&base).unwrap();

    // Working tree diff does NOT show untracked files - this is expected behavior
    // The new files become untracked after reset, and git diff HEAD ignores them
    let diff = repo.git.get_working_tree_diff().unwrap();

    // The diff should be empty or not contain the new file
    // (this is WHY we need separate new file tracking)
    assert!(
        !diff.contains("new_file.rs"),
        "Untracked files should NOT appear in git diff HEAD - this is why we track them separately"
    );
}

#[test]
fn test_working_tree_diff_shows_modifications() {
    let repo = TestRepo::new();

    // Create initial commit with content
    repo.write_file("src/main.rs", "fn main() {\n    // original\n}\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Modify the file
    repo.write_file(
        "src/main.rs",
        "fn main() {\n    println!(\"modified\");\n}\n",
    );
    repo.stage_all();
    repo.commit("Modify main.rs");

    // Reset to base
    repo.git.reset_to(&base).unwrap();

    // Working tree diff should show the modification
    let diff = repo.git.get_working_tree_diff().unwrap();
    assert!(diff.contains("-    // original"));
    assert!(diff.contains("+    println!(\"modified\")"));
}

// ============================================================================
// Multiple Hunks Per File Tests
// ============================================================================

#[test]
fn test_multiple_hunks_same_file_applied_together() {
    let repo = TestRepo::new();

    // Create initial commit with a larger file
    repo.write_file(
        "src/main.rs",
        r#"fn main() {
    // start
}

fn helper_one() {
    // helper one
}

fn helper_two() {
    // helper two
}
"#,
    );
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Modify multiple non-contiguous sections (creates multiple hunks)
    repo.write_file(
        "src/main.rs",
        r#"fn main() {
    println!("start");
}

fn helper_one() {
    println!("helper one");
}

fn helper_two() {
    println!("helper two");
}
"#,
    );
    repo.stage_all();
    repo.commit("Add print statements");

    // Reset to base
    repo.git.reset_to(&base).unwrap();

    // Parse hunks from working tree
    let diff = repo.git.get_working_tree_diff().unwrap();
    let hunks = git_reabsorb::diff_parser::parse_diff(&diff, &[], 0).unwrap();

    // Should have multiple hunks (depending on git's diff algorithm)
    // The key is that they all apply cleanly when grouped
    assert!(!hunks.is_empty());

    // Apply all hunks together
    let hunk_refs: Vec<&Hunk> = hunks.iter().collect();
    let result = repo.git.apply_hunks_to_index(&hunk_refs);
    assert!(
        result.is_ok(),
        "Multiple hunks should apply cleanly: {:?}",
        result
    );

    // Commit to verify everything staged correctly
    let sha = repo.git.commit("Test commit", false).unwrap();
    assert!(!sha.is_empty());
}

#[test]
fn test_hunks_sorted_by_line_number_before_apply() {
    let repo = TestRepo::new();

    // Create a file with distinct sections
    repo.write_file(
        "src/main.rs",
        r#"// Line 1
// Line 2
// Line 3
// Line 10
// Line 11
// Line 12
// Line 20
// Line 21
// Line 22
"#,
    );
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Modify lines in multiple places
    repo.write_file(
        "src/main.rs",
        r#"// Modified Line 1
// Line 2
// Line 3
// Line 10
// Modified Line 11
// Line 12
// Line 20
// Line 21
// Modified Line 22
"#,
    );
    repo.stage_all();
    repo.commit("Modify multiple lines");

    // Reset to base
    repo.git.reset_to(&base).unwrap();

    // Parse hunks
    let diff = repo.git.get_working_tree_diff().unwrap();
    let hunks = git_reabsorb::diff_parser::parse_diff(&diff, &[], 0).unwrap();

    // Even if hunks come in any order, they should apply correctly
    // because apply_hunks_to_index sorts them
    let hunk_refs: Vec<&Hunk> = hunks.iter().collect();
    let result = repo.git.apply_hunks_to_index(&hunk_refs);
    assert!(result.is_ok(), "Hunks should be sorted and apply cleanly");
}

// ============================================================================
// Full Reabsorb Flow with New Files Tests
// ============================================================================

#[test]
fn test_reabsorb_includes_new_files_in_squash() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // First commit: modify existing file
    repo.write_file("README.md", "# Test Project\n\nThis is a test.\n");
    repo.stage_all();
    repo.commit("Update README");

    // Second commit: add new files
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.write_file("src/lib.rs", "pub fn lib() {}\n");
    repo.stage_all();
    repo.commit("Add source files");

    // Get source commits for context
    let source_commits = repo.read_commits(&base, "HEAD");
    assert_eq!(source_commits.len(), 2);

    // Build new files mapping
    let mut new_files_to_commits = std::collections::HashMap::new();
    for commit in &source_commits {
        let new_files = repo.git.get_new_files_in_commit(&commit.sha).unwrap();
        for file in new_files {
            new_files_to_commits
                .entry(file)
                .or_insert_with(Vec::new)
                .push(commit.sha.clone());
        }
    }

    // Should have detected src/main.rs and src/lib.rs as new
    assert!(new_files_to_commits.contains_key("src/main.rs"));
    assert!(new_files_to_commits.contains_key("src/lib.rs"));

    // Now do the reset and verify new files become untracked
    repo.git.reset_to(&base).unwrap();

    // Check git status for untracked files - they appear under src/ directory
    let status = run_git(&repo.path, &["status", "--porcelain"]);
    // Git may show them as "?? src/" if the directory is new, or individually
    assert!(
        status.contains("?? src/") || status.contains("?? src/main.rs"),
        "New files should be untracked after reset. Status: {}",
        status
    );

    // Stage the new files
    run_git(&repo.path, &["add", "src/"]);

    // Verify they're now staged
    let status_after = run_git(&repo.path, &["status", "--porcelain"]);
    assert!(
        status_after.contains("A  src/main.rs") || status_after.contains("A src/main.rs"),
        "Files should be staged. Status: {}",
        status_after
    );
}

#[test]
fn test_new_files_mapped_to_correct_source_commits() {
    let repo = TestRepo::new();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Commit 1: adds file A
    repo.write_file("src/a.rs", "pub fn a() {}\n");
    repo.stage_all();
    let commit1 = repo.commit("Add a.rs");

    // Commit 2: adds file B
    repo.write_file("src/b.rs", "pub fn b() {}\n");
    repo.stage_all();
    let commit2 = repo.commit("Add b.rs");

    // Commit 3: adds file C
    repo.write_file("src/c.rs", "pub fn c() {}\n");
    repo.stage_all();
    let commit3 = repo.commit("Add c.rs");

    // Check each commit's new files
    let new_files_1 = repo.git.get_new_files_in_commit(&commit1).unwrap();
    let new_files_2 = repo.git.get_new_files_in_commit(&commit2).unwrap();
    let new_files_3 = repo.git.get_new_files_in_commit(&commit3).unwrap();

    assert_eq!(new_files_1, vec!["src/a.rs"]);
    assert_eq!(new_files_2, vec!["src/b.rs"]);
    assert_eq!(new_files_3, vec!["src/c.rs"]);

    // Verify using read_commits flow
    let source_commits = repo.read_commits(&base, "HEAD");
    assert_eq!(source_commits.len(), 3);
}

// ============================================================================
// Commit with no-verify Tests
// ============================================================================

#[test]
fn test_commit_without_no_verify() {
    let repo = TestRepo::new();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    repo.commit("Initial commit");

    // Add a file and stage it
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();

    // Commit without no_verify
    let result = repo.git.commit("Add main.rs", false);
    assert!(result.is_ok());
}

#[test]
fn test_commit_with_no_verify() {
    let repo = TestRepo::new();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    repo.commit("Initial commit");

    // Create a pre-commit hook that would fail
    let hooks_dir = repo.path.join(".git/hooks");
    fs::create_dir_all(&hooks_dir).unwrap();
    let hook_path = hooks_dir.join("pre-commit");
    fs::write(&hook_path, "#!/bin/sh\nexit 1\n").unwrap();

    // Make hook executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms).unwrap();
    }

    // Add a file and stage it
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();

    // Without no_verify, commit should fail (hook returns exit 1)
    let _result_without = repo.git.commit("Should fail", false);

    // Re-stage if needed (commit failure might unstage)
    repo.stage_all();

    // With no_verify, commit should succeed
    let result_with = repo.git.commit("Should succeed", true);
    assert!(
        result_with.is_ok(),
        "Commit with --no-verify should skip hooks"
    );
}

// ============================================================================
// Apply Hunks to Index Tests
// ============================================================================

#[test]
fn test_apply_single_hunk_to_index() {
    let repo = TestRepo::new();

    // Create initial file
    repo.write_file("src/main.rs", "fn main() {\n    // original\n}\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Modify the file
    repo.write_file(
        "src/main.rs",
        "fn main() {\n    println!(\"modified\");\n}\n",
    );
    repo.stage_all();
    repo.commit("Modify");

    // Reset to base
    repo.git.reset_to(&base).unwrap();

    // Parse the hunk
    let diff = repo.git.get_working_tree_diff().unwrap();
    let hunks = git_reabsorb::diff_parser::parse_diff(&diff, &[], 0).unwrap();
    assert!(!hunks.is_empty());

    // Apply single hunk
    let result = repo.git.apply_hunk_to_index(&hunks[0]);
    assert!(result.is_ok());

    // Verify something is staged
    let status = run_git(&repo.path, &["status", "--porcelain"]);
    assert!(status.contains("M") || status.contains("A"));
}

#[test]
fn test_apply_hunks_from_multiple_files() {
    let repo = TestRepo::new();

    // Create initial files
    repo.write_file("src/a.rs", "// a\n");
    repo.write_file("src/b.rs", "// b\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Modify both files
    repo.write_file("src/a.rs", "// modified a\n");
    repo.write_file("src/b.rs", "// modified b\n");
    repo.stage_all();
    repo.commit("Modify both");

    // Reset to base
    repo.git.reset_to(&base).unwrap();

    // Parse hunks
    let diff = repo.git.get_working_tree_diff().unwrap();
    let hunks = git_reabsorb::diff_parser::parse_diff(&diff, &[], 0).unwrap();
    assert_eq!(hunks.len(), 2);

    // Apply all hunks together
    let hunk_refs: Vec<&Hunk> = hunks.iter().collect();
    let result = repo.git.apply_hunks_to_index(&hunk_refs);
    assert!(result.is_ok());

    // Commit to verify
    let sha = repo.git.commit("Test", false).unwrap();
    assert!(!sha.is_empty());
}

// ============================================================================
// Plan File Tests
// ============================================================================

use git_reabsorb::models::{CommitDescription, HunkId, PlannedChange, PlannedCommit};
use git_reabsorb::reorganize::{delete_plan, has_saved_plan, load_plan, save_plan, SavedPlan};

const TEST_REF_NAMESPACE: &str = "test-branch";

fn test_pre_reabsorb_ref() -> String {
    git_reabsorb::git::pre_reabsorb_ref_for(TEST_REF_NAMESPACE)
}
use std::collections::HashMap;

#[test]
fn test_saved_plan_creation_and_roundtrip() {
    let repo = TestRepo::new();

    // Create initial commit (base)
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create a commit to reabsorb
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let head = repo.commit("Add main");

    // Read commits and hunks
    let commits = repo.read_commits(&base, &head);
    let hunks = repo.read_hunks(&commits);

    // Create planned commits
    let planned = vec![PlannedCommit::new(
        CommitDescription::new("Test commit", "This is a test commit"),
        vec![PlannedChange::ExistingHunk(hunks[0].id)],
    )];

    // Create SavedPlan
    let saved_plan = SavedPlan::new(
        "preserve".to_string(),
        base.clone(),
        head.clone(),
        &planned,
        &hunks,
        &[],
        &HashMap::new(),
        &HashMap::new(),
    );

    assert_eq!(saved_plan.version, 1);
    assert_eq!(saved_plan.strategy, "preserve");
    assert_eq!(saved_plan.base_sha, base);
    assert_eq!(saved_plan.original_head, head);
    assert_eq!(saved_plan.commits.len(), 1);
    assert_eq!(saved_plan.next_commit_index, 0);
    assert!(!saved_plan.is_complete());

    // Roundtrip to PlannedCommits
    let restored = saved_plan.to_planned_commits();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].description.short, "Test commit");
    assert_eq!(restored[0].changes.len(), 1);
}

#[test]
fn test_save_and_load_plan() {
    let repo = TestRepo::new();
    let namespace = format!("plan-{}", uuid());

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create a commit
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let head = repo.commit("Add main");

    // Read hunks
    let commits = repo.read_commits(&base, &head);
    let hunks = repo.read_hunks(&commits);

    // Create and save plan
    let planned = vec![PlannedCommit::new(
        CommitDescription::new("Commit 1", "First commit"),
        vec![PlannedChange::ExistingHunk(hunks[0].id)],
    )];

    let plan = SavedPlan::new(
        "by-file".to_string(),
        base.clone(),
        head.clone(),
        &planned,
        &hunks,
        &[],
        &HashMap::new(),
        &HashMap::new(),
    );

    // Save plan
    let path = save_plan(&namespace, &plan).unwrap();
    assert!(path.exists());
    assert!(has_saved_plan(&namespace));

    // Load plan
    let loaded = load_plan(&namespace).unwrap();
    assert_eq!(loaded.strategy, "by-file");
    assert_eq!(loaded.base_sha, base);
    assert_eq!(loaded.commits.len(), 1);

    // Clean up
    delete_plan(&namespace).unwrap();
    assert!(!has_saved_plan(&namespace));
}

#[test]
fn test_plan_progress_tracking() {
    let repo = TestRepo::new();

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create commits
    repo.write_file("src/a.rs", "// a\n");
    repo.stage_all();
    repo.commit("Add a");

    repo.write_file("src/b.rs", "// b\n");
    repo.stage_all();
    let head = repo.commit("Add b");

    // Read hunks
    let commits = repo.read_commits(&base, &head);
    let hunks = repo.read_hunks(&commits);

    // Create plan with 2 commits
    let planned = vec![
        PlannedCommit::new(
            CommitDescription::new("Commit 1", "First"),
            vec![PlannedChange::ExistingHunk(HunkId(0))],
        ),
        PlannedCommit::new(
            CommitDescription::new("Commit 2", "Second"),
            vec![PlannedChange::ExistingHunk(HunkId(1))],
        ),
    ];

    let mut plan = SavedPlan::new(
        "preserve".to_string(),
        base,
        head,
        &planned,
        &hunks,
        &[],
        &HashMap::new(),
        &HashMap::new(),
    );

    // Initially not complete
    assert!(!plan.is_complete());
    assert_eq!(plan.next_commit_index, 0);
    assert_eq!(plan.remaining_commits().len(), 2);

    // Mark first commit as created
    plan.mark_commit_created("abc123".to_string());
    assert!(!plan.is_complete());
    assert_eq!(plan.next_commit_index, 1);
    assert_eq!(plan.remaining_commits().len(), 1);
    assert_eq!(plan.commits[0].created_sha, Some("abc123".to_string()));

    // Mark second commit as created
    plan.mark_commit_created("def456".to_string());
    assert!(plan.is_complete());
    assert_eq!(plan.next_commit_index, 2);
    assert_eq!(plan.remaining_commits().len(), 0);
    assert_eq!(plan.commits[1].created_sha, Some("def456".to_string()));
}

#[test]
fn test_plan_with_new_hunks() {
    let repo = TestRepo::new();
    let namespace = format!("plan-{}", uuid());

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create a commit
    repo.write_file("src/main.rs", "fn main() {\n    println!(\"hello\");\n}\n");
    repo.stage_all();
    let head = repo.commit("Add main");

    // Read hunks
    let commits = repo.read_commits(&base, &head);
    let hunks = repo.read_hunks(&commits);

    // Create a "new" hunk (simulating LLM splitting)
    let mut new_hunk = hunks[0].clone();
    new_hunk.id = HunkId(100);

    // Create plan with both existing and new hunks
    let planned = vec![PlannedCommit::new(
        CommitDescription::new("Split commit", "Contains new hunk"),
        vec![
            PlannedChange::ExistingHunk(hunks[0].id),
            PlannedChange::NewHunk(new_hunk.clone()),
        ],
    )];

    let plan = SavedPlan::new(
        "llm".to_string(),
        base.clone(),
        head.clone(),
        &planned,
        &hunks,
        &[new_hunk],
        &HashMap::new(),
        &HashMap::new(),
    );

    // Save and reload
    save_plan(&namespace, &plan).unwrap();
    let loaded = load_plan(&namespace).unwrap();

    // Verify new hunks are preserved
    assert_eq!(loaded.new_hunks.len(), 1);
    assert_eq!(loaded.new_hunks[0].id, 100);

    // Roundtrip should work
    let restored = loaded.to_planned_commits();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].changes.len(), 2);

    // Clean up
    delete_plan(&namespace).unwrap();
}

#[test]
fn test_plan_stores_file_mappings() {
    let repo = TestRepo::new();
    let namespace = format!("plan-{}", uuid());

    // Create initial commit
    repo.write_file("README.md", "# Test\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create a commit
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let head = repo.commit("Add main");

    // Read hunks
    let commits = repo.read_commits(&base, &head);
    let hunks = repo.read_hunks(&commits);

    // Create file mappings
    let mut file_to_commits = HashMap::new();
    file_to_commits.insert("src/main.rs".to_string(), vec![head.clone()]);

    let mut new_files_to_commits = HashMap::new();
    new_files_to_commits.insert("src/main.rs".to_string(), vec![head.clone()]);

    let planned = vec![PlannedCommit::new(
        CommitDescription::new("Test", "Test"),
        vec![PlannedChange::ExistingHunk(hunks[0].id)],
    )];

    let plan = SavedPlan::new(
        "preserve".to_string(),
        base,
        head.clone(),
        &planned,
        &hunks,
        &[],
        &file_to_commits,
        &new_files_to_commits,
    );

    // Save and reload
    save_plan(&namespace, &plan).unwrap();
    let loaded = load_plan(&namespace).unwrap();

    // Verify mappings are restored
    let restored_file_to_commits = loaded.get_file_to_commits();
    let restored_new_files = loaded.get_new_files_to_commits();

    assert_eq!(restored_file_to_commits.len(), 1);
    assert_eq!(
        restored_file_to_commits.get("src/main.rs"),
        Some(&vec![head.clone()])
    );

    assert_eq!(restored_new_files.len(), 1);
    assert_eq!(restored_new_files.get("src/main.rs"), Some(&vec![head]));

    // Clean up
    delete_plan(&namespace).unwrap();
}

// ============================================================================
// End-to-End Split Commit Tests
// ============================================================================

/// Test that changes within a single file from one commit can be split into
/// two separate commits by manually selecting hunks.
#[test]
fn test_split_single_file_commit_into_two_commits() {
    let repo = TestRepo::new();

    // Create initial file with multiple functions, well-separated
    // to ensure git creates separate hunks
    repo.write_file(
        "src/main.rs",
        r#"fn main() {
    // main entry point
}

// spacing
//
//
//
//
//
//
//
//
//

fn helper_one() {
    // first helper
}

fn helper_two() {
    // second helper
}
"#,
    );
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create a single commit that modifies multiple non-contiguous sections
    // This should create multiple hunks in the same file
    repo.write_file(
        "src/main.rs",
        r#"fn main() {
    println!("Hello from main!");
}

// spacing
//
//
//
//
//
//
//
//
//

fn helper_one() {
    println!("Hello from helper one!");
}

fn helper_two() {
    // second helper
}
"#,
    );
    repo.stage_all();
    let _original_commit = repo.commit("Add print statements to main and helper_one");

    // Now we'll simulate the reabsorb flow:
    // 1. Reset to base
    // 2. Parse working tree diff to get hunks
    // 3. Apply hunks selectively to create two commits

    // Reset to base (keeps changes in working tree)
    repo.git.reset_to(&base).unwrap();

    // Parse hunks from working tree diff
    let diff = repo.git.get_working_tree_diff().unwrap();
    let hunks = git_reabsorb::diff_parser::parse_diff(&diff, &[], 0).unwrap();

    // We need at least 2 hunks to test splitting
    assert!(
        hunks.len() >= 2,
        "Expected at least 2 hunks to test splitting, got {}",
        hunks.len()
    );

    // Split into two groups
    let first_group: Vec<&git_reabsorb::models::Hunk> = vec![&hunks[0]];
    let second_group: Vec<&git_reabsorb::models::Hunk> = hunks.iter().skip(1).collect();

    // Apply first group and commit
    repo.git.apply_hunks_to_index(&first_group).unwrap();
    let first_sha = repo.git.commit("First split commit", false).unwrap();
    assert!(!first_sha.is_empty());

    // Apply second group and commit
    repo.git.apply_hunks_to_index(&second_group).unwrap();
    let second_sha = repo.git.commit("Second split commit", false).unwrap();
    assert!(!second_sha.is_empty());

    // Verify we have two distinct commits
    assert_ne!(first_sha, second_sha);

    // Verify the commit history
    let commits = repo.read_commits(&base, "HEAD");
    assert_eq!(
        commits.len(),
        2,
        "Should have exactly 2 commits after splitting"
    );
    assert_eq!(commits[0].short_description, "First split commit");
    assert_eq!(commits[1].short_description, "Second split commit");
}

/// Test splitting changes across two separate functions in the same file
/// into two distinct commits - one per function.
#[test]
fn test_split_file_changes_by_function() {
    let repo = TestRepo::new();

    // Create a file with two well-separated functions (many lines apart)
    // to ensure git creates separate hunks
    repo.write_file(
        "src/lib.rs",
        r#"// Top of file
fn function_a() {
    // placeholder a
}

// Lots of spacing to ensure separate hunks
//
//
//
//
//
//
//
//
//
//

fn function_b() {
    // placeholder b
}
// End of file
"#,
    );
    repo.stage_all();
    let base = repo.commit("Initial file with two functions");

    // Modify both functions in a single commit
    repo.write_file(
        "src/lib.rs",
        r#"// Top of file
fn function_a() {
    println!("Function A implementation");
}

// Lots of spacing to ensure separate hunks
//
//
//
//
//
//
//
//
//
//

fn function_b() {
    println!("Function B implementation");
}
// End of file
"#,
    );
    repo.stage_all();
    let _combined_commit = repo.commit("Implement both functions");

    // Reset to base
    repo.git.reset_to(&base).unwrap();

    // Parse the diff
    let diff = repo.git.get_working_tree_diff().unwrap();
    let hunks = git_reabsorb::diff_parser::parse_diff(&diff, &[], 0).unwrap();

    // With enough spacing, we should get 2 separate hunks
    assert!(
        hunks.len() >= 2,
        "Expected at least 2 hunks for separate functions, got {}",
        hunks.len()
    );

    // Commit function_a changes first
    let func_a_hunks: Vec<&git_reabsorb::models::Hunk> = vec![&hunks[0]];
    repo.git.apply_hunks_to_index(&func_a_hunks).unwrap();
    let commit_a = repo.git.commit("Implement function_a", false).unwrap();

    // Commit function_b changes second
    let func_b_hunks: Vec<&git_reabsorb::models::Hunk> = hunks.iter().skip(1).collect();
    repo.git.apply_hunks_to_index(&func_b_hunks).unwrap();
    let commit_b = repo.git.commit("Implement function_b", false).unwrap();

    // Verify
    assert_ne!(commit_a, commit_b);

    // Check the file content is correct after both commits
    let final_content = fs::read_to_string(repo.path.join("src/lib.rs")).unwrap();
    assert!(final_content.contains("Function A implementation"));
    assert!(final_content.contains("Function B implementation"));

    // Verify commit history
    let commits = repo.read_commits(&base, "HEAD");
    assert_eq!(commits.len(), 2);
}

/// Test the complete workflow: one old commit with changes to a file
/// gets reorganized into two new commits.
#[test]
fn test_end_to_end_reorganize_single_commit_to_multiple() {
    let repo = TestRepo::new();

    // Create initial file
    repo.write_file(
        "src/calculator.rs",
        r#"pub struct Calculator;

impl Calculator {
    pub fn add(&self, a: i32, b: i32) -> i32 {
        // TODO: implement
        0
    }

    pub fn subtract(&self, a: i32, b: i32) -> i32 {
        // TODO: implement
        0
    }
}
"#,
    );
    repo.stage_all();
    let base = repo.commit("Add Calculator struct with stubs");

    // Create ONE commit that implements BOTH methods
    repo.write_file(
        "src/calculator.rs",
        r#"pub struct Calculator;

impl Calculator {
    pub fn add(&self, a: i32, b: i32) -> i32 {
        a + b
    }

    pub fn subtract(&self, a: i32, b: i32) -> i32 {
        a - b
    }
}
"#,
    );
    repo.stage_all();
    let original_head = repo.commit("Implement add and subtract methods");

    // Read the original commits to get source info
    let source_commits = repo.read_commits(&base, &original_head);
    assert_eq!(source_commits.len(), 1);
    assert_eq!(
        source_commits[0].short_description,
        "Implement add and subtract methods"
    );

    // Now reorganize: reset and split
    repo.git.reset_to(&base).unwrap();

    // Get hunks from working tree
    let diff = repo.git.get_working_tree_diff().unwrap();
    let hunks = git_reabsorb::diff_parser::parse_diff(&diff, &[original_head.clone()], 0).unwrap();

    // The hunks should reference the original commit
    for hunk in &hunks {
        assert!(
            hunk.likely_source_commits.contains(&original_head),
            "Hunks should track their source commit"
        );
    }

    // Apply all hunks and create new commits based on some criteria
    // For this test, we'll just verify the mechanics work
    let hunk_refs: Vec<&git_reabsorb::models::Hunk> = hunks.iter().collect();
    repo.git.apply_hunks_to_index(&hunk_refs).unwrap();
    let new_sha = repo.git.commit("Reorganized: implement both methods", false).unwrap();

    // Verify the new commit exists and file content is correct
    assert!(!new_sha.is_empty());

    let final_content = fs::read_to_string(repo.path.join("src/calculator.rs")).unwrap();
    assert!(final_content.contains("a + b"));
    assert!(final_content.contains("a - b"));
    assert!(!final_content.contains("TODO: implement"));

    // The new commit should be different from the original
    // (same content but different SHA due to different message/timestamp)
    let new_commits = repo.read_commits(&base, "HEAD");
    assert_eq!(new_commits.len(), 1);
    assert_eq!(
        new_commits[0].short_description,
        "Reorganized: implement both methods"
    );
}

// ============================================================================
// Hierarchical Reorganizer Tests
// ============================================================================

/// Test the hierarchical reorganizer with heuristics only (no LLM)
#[test]
fn test_hierarchical_reorganizer_heuristic_mode() {
    let repo = TestRepo::new();

    // Create initial setup
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create a commit with changes across multiple files and categories
    repo.write_file(
        "src/auth/login.rs",
        r#"pub fn login(user: &str) -> bool {
    // Authenticate user
    true
}
"#,
    );
    repo.write_file(
        "src/auth/logout.rs",
        r#"pub fn logout() {
    // Clear session
}
"#,
    );
    repo.write_file(
        "tests/auth_test.rs",
        r#"#[test]
fn test_login() {
    assert!(login("user"));
}
"#,
    );
    repo.write_file("README.md", "# Auth Module\n\nAuthentication module.\n");
    repo.stage_all();
    let head = repo.commit("Add authentication module");

    // Read commits and hunks
    let source_commits = repo.read_commits(&base, &head);
    let hunks = repo.read_hunks(&source_commits);

    assert!(!hunks.is_empty(), "Should have hunks to reorganize");

    // Run hierarchical reorganizer in heuristic mode
    let config = HierarchicalConfig::heuristic_only();
    let reorganizer = HierarchicalReorganizer::new(None).with_config(config);

    let result = reorganizer.reorganize(&source_commits, &hunks);

    assert!(result.is_ok(), "Reorganization should succeed: {:?}", result.err());
    let planned_commits = result.unwrap();

    // Should have at least one commit
    assert!(!planned_commits.is_empty(), "Should have planned commits");

    // All hunks should be assigned
    let total_changes: usize = planned_commits.iter().map(|c| c.changes.len()).sum();
    assert_eq!(total_changes, hunks.len(), "All hunks should be assigned");

    // Each commit should have a non-empty message
    for commit in &planned_commits {
        assert!(
            !commit.description.short.is_empty(),
            "Commit should have a short description"
        );
    }
}

/// Test that hierarchical reorganizer properly groups changes by topic
#[test]
fn test_hierarchical_reorganizer_topic_grouping() {
    let repo = TestRepo::new();

    // Create initial setup
    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let base = repo.commit("Initial commit");

    // Create changes in different topic areas with clear separation
    repo.write_file(
        "src/api/users.rs",
        r#"// Users API
pub fn get_user() {}
pub fn create_user() {}
"#,
    );
    repo.write_file(
        "src/api/posts.rs",
        r#"// Posts API - completely different topic


// Many lines of spacing to ensure separate topic


pub fn get_posts() {}
pub fn create_post() {}
"#,
    );
    repo.stage_all();
    let head = repo.commit("Add API endpoints");

    let source_commits = repo.read_commits(&base, &head);
    let hunks = repo.read_hunks(&source_commits);

    let config = HierarchicalConfig::heuristic_only();
    let reorganizer = HierarchicalReorganizer::new(None).with_config(config);

    let planned_commits = reorganizer.reorganize(&source_commits, &hunks).unwrap();

    // All hunks should be assigned
    let total_changes: usize = planned_commits.iter().map(|c| c.changes.len()).sum();
    assert_eq!(total_changes, hunks.len());
}

/// Test hierarchical reorganizer with mixed change categories
#[test]
fn test_hierarchical_reorganizer_category_ordering() {
    let repo = TestRepo::new();

    repo.write_file("src/lib.rs", "// lib\n");
    repo.stage_all();
    let base = repo.commit("Initial");

    // Add changes of different categories
    repo.write_file("Cargo.toml", "[package]\nname = \"test\"\n");
    repo.write_file(
        "src/lib.rs",
        r#"// lib
pub fn feature() {}
"#,
    );
    repo.write_file(
        "tests/test.rs",
        r#"#[test]
fn test_feature() {}
"#,
    );
    repo.stage_all();
    let head = repo.commit("Mixed changes");

    let source_commits = repo.read_commits(&base, &head);
    let hunks = repo.read_hunks(&source_commits);

    let config = HierarchicalConfig::heuristic_only();
    let reorganizer = HierarchicalReorganizer::new(None).with_config(config);

    let planned_commits = reorganizer.reorganize(&source_commits, &hunks).unwrap();

    // Should have at least one commit
    assert!(!planned_commits.is_empty());

    // All hunks should be assigned
    let total_changes: usize = planned_commits.iter().map(|c| c.changes.len()).sum();
    assert_eq!(total_changes, hunks.len());
}

/// Test that hierarchical reorganizer handles single-file changes
#[test]
fn test_hierarchical_reorganizer_single_file() {
    let repo = TestRepo::new();

    repo.write_file("src/main.rs", "fn main() {}\n");
    repo.stage_all();
    let base = repo.commit("Initial");

    repo.write_file(
        "src/main.rs",
        r#"fn main() {
    println!("Hello");
}

fn helper() {}
"#,
    );
    repo.stage_all();
    let head = repo.commit("Update main");

    let source_commits = repo.read_commits(&base, &head);
    let hunks = repo.read_hunks(&source_commits);

    let config = HierarchicalConfig::heuristic_only();
    let reorganizer = HierarchicalReorganizer::new(None).with_config(config);

    let planned_commits = reorganizer.reorganize(&source_commits, &hunks).unwrap();

    assert!(!planned_commits.is_empty());

    // All changes should be assigned
    let total_changes: usize = planned_commits.iter().map(|c| c.changes.len()).sum();
    assert_eq!(total_changes, hunks.len());
}
