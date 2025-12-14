//! Unified diff patch parsing, generation, and application.

mod context;
mod parser;
mod writer;

pub use context::PatchContext;
pub use writer::PatchWriter;

use crate::models::{FileChange, Hunk};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Invalid hunk header: {0}")]
    InvalidHunkHeader(String),
    #[error("Unexpected diff format: {0}")]
    UnexpectedFormat(String),
}

#[derive(Debug, Default)]
pub struct Patch {
    pub hunks: Vec<Hunk>,
    pub file_changes: Vec<FileChange>,
}

pub fn parse(
    diff_output: &str,
    likely_source_commits: &[String],
    hunk_id_start: usize,
) -> Result<Patch, ParseError> {
    parser::PatchParser::new(likely_source_commits, hunk_id_start).parse(diff_output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ChangeType;
    use std::path::PathBuf;

    #[test]
    fn test_parse_simple_diff() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
index 1234567..abcdefg 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
     println!("World");
 }
"#;

        let hunks = parse(diff, &["abc123".to_string()], 0).unwrap().hunks;
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].file_path, PathBuf::from("src/main.rs"));
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 3);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 4);
        assert_eq!(hunks[0].likely_source_commits, vec!["abc123".to_string()]);
    }

    #[test]
    fn test_parse_diff_multiple_source_commits() {
        let diff = r#"diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1,2 +1,3 @@
 line1
+line2
 line3
"#;

        let source_commits = vec!["commit1".to_string(), "commit2".to_string()];
        let hunks = parse(diff, &source_commits, 0).unwrap().hunks;
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].likely_source_commits, source_commits);
    }

    #[test]
    fn test_parse_diff_multiple_hunks_same_file() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("start");
 }

@@ -10,3 +11,4 @@
 fn helper() {
+    println!("helper");
 }
"#;

        let hunks = parse(diff, &[], 0).unwrap().hunks;
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].file_path, hunks[1].file_path);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[1].old_start, 10);
    }

    #[test]
    fn test_parse_diff_multiple_files() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,2 +1,3 @@
 fn main() {
+    lib::greet();
 }
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,4 @@
 pub fn greet() {
+    println!("Hello");
 }
"#;

        let hunks = parse(diff, &[], 0).unwrap().hunks;
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].file_path, PathBuf::from("src/main.rs"));
        assert_eq!(hunks[1].file_path, PathBuf::from("src/lib.rs"));
    }

    #[test]
    fn test_parse_diff_new_file() {
        let diff = r#"diff --git a/src/new.rs b/src/new.rs
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,3 @@
+fn new_function() {
+    println!("I'm new!");
+}
"#;

        let hunks = parse(diff, &[], 0).unwrap().hunks;
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].file_path, PathBuf::from("src/new.rs"));
        assert_eq!(hunks[0].old_start, 0);
        assert_eq!(hunks[0].old_count, 0);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 3);
    }

    #[test]
    fn test_parse_diff_deleted_file() {
        let diff = r#"diff --git a/src/old.rs b/src/old.rs
deleted file mode 100644
index 1234567..0000000
--- a/src/old.rs
+++ /dev/null
@@ -1,3 +0,0 @@
-fn old_function() {
-    println!("I'm being deleted!");
-}
"#;

        let hunks = parse(diff, &[], 0).unwrap().hunks;
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_count, 3);
        assert_eq!(hunks[0].new_start, 0);
        assert_eq!(hunks[0].new_count, 0);
    }

    #[test]
    fn test_parse_diff_with_context_function_header() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -5,6 +5,7 @@ fn some_function() {
     let x = 1;
     let y = 2;
+    let z = 3;
     println!("{}", x + y);
 }
"#;

        let hunks = parse(diff, &[], 0).unwrap().hunks;
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 5);
        assert_eq!(hunks[0].old_count, 6);
        assert_eq!(hunks[0].new_start, 5);
        assert_eq!(hunks[0].new_count, 7);
    }

    #[test]
    fn test_parse_diff_empty_source_commits() {
        let diff = r#"diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1 +1,2 @@
 line1
+line2
"#;

        let hunks = parse(diff, &[], 0).unwrap().hunks;
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].likely_source_commits.is_empty());
    }

    #[test]
    fn test_hunk_id_starts_from_provided_value() {
        let diff = r#"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1 +1,2 @@
 line
+new
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
@@ -1 +1,2 @@
 line
+new
"#;

        let hunks = parse(diff, &[], 100).unwrap().hunks;
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].id.0, 100);
        assert_eq!(hunks[1].id.0, 101);
    }

    #[test]
    fn test_parse_binary_file_new() {
        let diff = r#"diff --git a/image.png b/image.png
new file mode 100644
index 0000000..abcdefg
Binary files /dev/null and b/image.png differ
"#;

        let result = parse(diff, &["commit1".to_string()], 0).unwrap();
        assert_eq!(result.hunks.len(), 0);
        assert_eq!(result.file_changes.len(), 1);
        assert_eq!(result.file_changes[0].file_path, PathBuf::from("image.png"));
        assert_eq!(result.file_changes[0].change_type, ChangeType::Added);
        assert!(result.file_changes[0].is_binary);
        assert_eq!(
            result.file_changes[0].likely_source_commits,
            vec!["commit1".to_string()]
        );
    }

    #[test]
    fn test_parse_binary_file_modified() {
        let diff = r#"diff --git a/image.png b/image.png
index 1234567..abcdefg 100644
Binary files a/image.png and b/image.png differ
"#;

        let result = parse(diff, &[], 0).unwrap();
        assert_eq!(result.hunks.len(), 0);
        assert_eq!(result.file_changes.len(), 1);
        assert_eq!(result.file_changes[0].file_path, PathBuf::from("image.png"));
        assert_eq!(result.file_changes[0].change_type, ChangeType::Modified);
        assert!(result.file_changes[0].is_binary);
    }

    #[test]
    fn test_parse_binary_file_deleted() {
        let diff = r#"diff --git a/image.png b/image.png
deleted file mode 100644
index abcdefg..0000000
Binary files a/image.png and /dev/null differ
"#;

        let result = parse(diff, &[], 0).unwrap();
        assert_eq!(result.hunks.len(), 0);
        assert_eq!(result.file_changes.len(), 1);
        assert_eq!(result.file_changes[0].file_path, PathBuf::from("image.png"));
        assert_eq!(result.file_changes[0].change_type, ChangeType::Deleted);
        assert!(result.file_changes[0].is_binary);
    }

    #[test]
    fn test_parse_mixed_text_and_binary() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,2 +1,3 @@
 fn main() {
+    println!("hello");
 }
diff --git a/image.png b/image.png
new file mode 100644
index 0000000..abcdefg
Binary files /dev/null and b/image.png differ
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1,2 @@
 pub fn foo() {}
+pub fn bar() {}
"#;

        let result = parse(diff, &[], 0).unwrap();
        assert_eq!(result.hunks.len(), 2);
        assert_eq!(result.file_changes.len(), 1);
        assert_eq!(result.hunks[0].file_path, PathBuf::from("src/main.rs"));
        assert_eq!(result.hunks[1].file_path, PathBuf::from("src/lib.rs"));
        assert_eq!(result.file_changes[0].file_path, PathBuf::from("image.png"));
        assert!(result.file_changes[0].is_binary);
    }

    #[test]
    fn test_parse_mode_only_change() {
        let diff = r#"diff --git a/script.sh b/script.sh
old mode 100644
new mode 100755
"#;

        let result = parse(diff, &["commit1".to_string()], 0).unwrap();
        assert_eq!(result.hunks.len(), 0);
        assert_eq!(result.file_changes.len(), 1);
        assert_eq!(result.file_changes[0].file_path, PathBuf::from("script.sh"));
        assert_eq!(result.file_changes[0].change_type, ChangeType::Modified);
        assert_eq!(result.file_changes[0].old_mode, Some("100644".to_string()));
        assert_eq!(result.file_changes[0].new_mode, Some("100755".to_string()));
        assert!(!result.file_changes[0].is_binary);
        assert!(!result.file_changes[0].has_content_hunks);
        assert_eq!(
            result.file_changes[0].likely_source_commits,
            vec!["commit1".to_string()]
        );
    }

    #[test]
    fn test_parse_mode_change_with_content_change() {
        let diff = r#"diff --git a/script.sh b/script.sh
old mode 100644
new mode 100755
index 1234567..abcdefg
--- a/script.sh
+++ b/script.sh
@@ -1 +1,2 @@
 echo "hello"
+echo "world"
"#;

        let result = parse(diff, &[], 0).unwrap();
        assert_eq!(result.hunks.len(), 1);
        assert_eq!(result.file_changes.len(), 1);
        assert_eq!(result.file_changes[0].file_path, PathBuf::from("script.sh"));
        assert_eq!(result.file_changes[0].change_type, ChangeType::Modified);
        assert_eq!(result.file_changes[0].old_mode, Some("100644".to_string()));
        assert_eq!(result.file_changes[0].new_mode, Some("100755".to_string()));
        assert!(result.file_changes[0].has_content_hunks);
    }

    #[test]
    fn test_parse_multiple_mode_changes() {
        let diff = r#"diff --git a/script1.sh b/script1.sh
old mode 100644
new mode 100755
diff --git a/script2.sh b/script2.sh
old mode 100644
new mode 100755
"#;

        let result = parse(diff, &[], 0).unwrap();
        assert_eq!(result.file_changes.len(), 2);
        assert_eq!(
            result.file_changes[0].file_path,
            PathBuf::from("script1.sh")
        );
        assert_eq!(
            result.file_changes[1].file_path,
            PathBuf::from("script2.sh")
        );
    }
}
