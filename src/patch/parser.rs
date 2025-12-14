//! Unified diff parsing.

use std::path::PathBuf;

use crate::models::{ChangeType, DiffLine, FileChange, Hunk, HunkId};

use super::{ParseError, Patch};

pub(super) struct PatchParser<'a> {
    result: Patch,
    likely_source_commits: &'a [String],
    next_hunk_id: usize,
    file: Option<FileChange>,
    hunk: Option<HunkBuilder>,
}

impl<'a> PatchParser<'a> {
    pub fn new(likely_source_commits: &'a [String], hunk_id_start: usize) -> Self {
        Self {
            result: Patch::default(),
            likely_source_commits,
            next_hunk_id: hunk_id_start,
            file: None,
            hunk: None,
        }
    }

    pub fn parse(mut self, diff_output: &str) -> Result<Patch, ParseError> {
        for line in diff_output.lines() {
            self.process_line(line)?;
        }
        self.finalize()
    }

    fn process_line(&mut self, line: &str) -> Result<(), ParseError> {
        if line.starts_with("diff --git ") {
            self.start_new_file(line);
            return Ok(());
        }

        if let Some(rest) = line.strip_prefix("new file mode ") {
            if let Some(ref mut file) = self.file {
                file.change_type = ChangeType::Added;
                file.new_mode = Some(rest.to_string());
            }
            return Ok(());
        }
        if line.starts_with("new file") {
            if let Some(ref mut file) = self.file {
                file.change_type = ChangeType::Added;
            }
            return Ok(());
        }
        if let Some(rest) = line.strip_prefix("deleted file mode ") {
            if let Some(ref mut file) = self.file {
                file.change_type = ChangeType::Deleted;
                file.old_mode = Some(rest.to_string());
            }
            return Ok(());
        }
        if line.starts_with("deleted file") {
            if let Some(ref mut file) = self.file {
                file.change_type = ChangeType::Deleted;
            }
            return Ok(());
        }
        if let Some(mode) = line.strip_prefix("old mode ") {
            if let Some(ref mut file) = self.file {
                file.old_mode = Some(mode.to_string());
            }
            return Ok(());
        }
        if let Some(mode) = line.strip_prefix("new mode ") {
            if let Some(ref mut file) = self.file {
                file.new_mode = Some(mode.to_string());
            }
            return Ok(());
        }

        if let Some(path) = line.strip_prefix("--- a/") {
            if let Some(ref mut file) = self.file {
                if file.change_type == ChangeType::Deleted {
                    file.file_path = PathBuf::from(path);
                }
            }
            return Ok(());
        }
        if line.starts_with("--- ") {
            return Ok(());
        }
        if let Some(path) = line.strip_prefix("+++ b/") {
            if let Some(ref mut file) = self.file {
                file.file_path = PathBuf::from(path);
            }
            return Ok(());
        }
        if line.starts_with("+++ ") {
            return Ok(());
        }

        if line.starts_with("Binary files") {
            if let Some(ref mut file) = self.file {
                file.is_binary = true;
            }
            return Ok(());
        }

        if line.starts_with("index ")
            || line.starts_with("similarity index")
            || line.starts_with("rename from")
            || line.starts_with("rename to")
        {
            return Ok(());
        }

        if line.starts_with("@@ ") {
            self.start_hunk(line)?;
            return Ok(());
        }

        self.process_hunk_line(line);
        Ok(())
    }

    fn start_new_file(&mut self, line: &str) {
        self.finalize_hunk();
        self.finalize_file();
        self.file = parse_header(line).map(FileChange::with_path);
    }

    fn start_hunk(&mut self, line: &str) -> Result<(), ParseError> {
        if let Some(ref mut file) = self.file {
            file.has_content_hunks = true;
        }
        self.finalize_hunk();

        let file_path = self
            .file
            .as_ref()
            .map(|f| f.file_path.clone())
            .unwrap_or_default();

        self.hunk = Some(
            HunkBuilder::new(HunkId(self.next_hunk_id))
                .with_file_path(file_path)
                .with_header(line)?,
        );
        self.next_hunk_id += 1;
        Ok(())
    }

    fn process_hunk_line(&mut self, line: &str) {
        let Some(ref mut builder) = self.hunk else {
            return;
        };

        if let Some(content) = line.strip_prefix('+') {
            builder.push_line(DiffLine::Added(content.to_string()));
        } else if let Some(content) = line.strip_prefix('-') {
            builder.push_line(DiffLine::Removed(content.to_string()));
        } else if let Some(content) = line.strip_prefix(' ') {
            builder.push_line(DiffLine::Context(content.to_string()));
        } else if line == "\\ No newline at end of file" {
            if let Some(last_line) = builder.last_line() {
                match last_line {
                    DiffLine::Removed(_) => builder.mark_old_missing_newline(),
                    DiffLine::Added(_) => builder.mark_new_missing_newline(),
                    DiffLine::Context(_) => {
                        builder.mark_old_missing_newline();
                        builder.mark_new_missing_newline();
                    }
                }
            }
        } else if line.is_empty() {
            builder.push_line(DiffLine::Context(String::new()));
        }
    }

    fn finalize_hunk(&mut self) {
        if let Some(builder) = self.hunk.take() {
            self.result
                .hunks
                .push(builder.build(self.likely_source_commits));
        }
    }

    fn finalize_file(&mut self) {
        let Some(file) = self.file.take() else {
            return;
        };

        let has_mode_info = file.old_mode.is_some() || file.new_mode.is_some();
        if !has_mode_info && !file.is_binary {
            return;
        }

        let (old_mode, new_mode) = match &file.change_type {
            ChangeType::Added => (None, file.new_mode),
            ChangeType::Deleted => (file.old_mode, None),
            ChangeType::Modified => (file.old_mode, file.new_mode),
        };

        self.result.file_changes.push(FileChange {
            file_path: file.file_path,
            change_type: file.change_type,
            old_mode,
            new_mode,
            is_binary: file.is_binary,
            has_content_hunks: file.has_content_hunks,
            likely_source_commits: self.likely_source_commits.to_vec(),
        });
    }

    fn finalize(mut self) -> Result<Patch, ParseError> {
        self.finalize_hunk();
        self.finalize_file();
        Ok(self.result)
    }
}

fn parse_header(line: &str) -> Option<PathBuf> {
    let rest = line.strip_prefix("diff --git ")?;
    let parts: Vec<&str> = rest.splitn(2, " b/").collect();
    if parts.len() == 2 {
        Some(PathBuf::from(parts[1]))
    } else {
        None
    }
}

fn parse_range(s: &str) -> Result<(u32, u32), ParseError> {
    if let Some((start, count)) = s.split_once(',') {
        let start: u32 = start
            .parse()
            .map_err(|_| ParseError::InvalidHunkHeader(s.to_string()))?;
        let count: u32 = count
            .parse()
            .map_err(|_| ParseError::InvalidHunkHeader(s.to_string()))?;
        Ok((start, count))
    } else {
        let start: u32 = s
            .parse()
            .map_err(|_| ParseError::InvalidHunkHeader(s.to_string()))?;
        Ok((start, 1))
    }
}

struct HunkBuilder {
    id: HunkId,
    file_path: PathBuf,
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
    lines: Vec<DiffLine>,
    old_missing_newline_at_eof: bool,
    new_missing_newline_at_eof: bool,
}

impl HunkBuilder {
    fn new(id: HunkId) -> Self {
        Self {
            id,
            file_path: PathBuf::new(),
            old_start: 0,
            old_count: 0,
            new_start: 0,
            new_count: 0,
            lines: Vec::new(),
            old_missing_newline_at_eof: false,
            new_missing_newline_at_eof: false,
        }
    }

    fn with_file_path(mut self, path: PathBuf) -> Self {
        self.file_path = path;
        self
    }

    fn with_header(mut self, line: &str) -> Result<Self, ParseError> {
        let content = line
            .strip_prefix("@@ ")
            .and_then(|s| s.split(" @@").next())
            .ok_or_else(|| ParseError::InvalidHunkHeader(line.to_string()))?;

        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.len() != 2 {
            return Err(ParseError::InvalidHunkHeader(line.to_string()));
        }

        let (old_start, old_count) = parse_range(parts[0].strip_prefix('-').unwrap_or(parts[0]))?;
        let (new_start, new_count) = parse_range(parts[1].strip_prefix('+').unwrap_or(parts[1]))?;

        self.old_start = old_start;
        self.old_count = old_count;
        self.new_start = new_start;
        self.new_count = new_count;
        Ok(self)
    }

    fn push_line(&mut self, line: DiffLine) {
        self.lines.push(line);
    }

    fn mark_old_missing_newline(&mut self) {
        self.old_missing_newline_at_eof = true;
    }

    fn mark_new_missing_newline(&mut self) {
        self.new_missing_newline_at_eof = true;
    }

    fn last_line(&self) -> Option<&DiffLine> {
        self.lines.last()
    }

    fn build(self, likely_source_commits: &[String]) -> Hunk {
        Hunk {
            id: self.id,
            file_path: self.file_path,
            old_start: self.old_start,
            old_count: self.old_count,
            new_start: self.new_start,
            new_count: self.new_count,
            lines: self.lines,
            likely_source_commits: likely_source_commits.to_vec(),
            old_missing_newline_at_eof: self.old_missing_newline_at_eof,
            new_missing_newline_at_eof: self.new_missing_newline_at_eof,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hunk_builder_with_header() {
        let builder = HunkBuilder::new(HunkId(0))
            .with_header("@@ -1,5 +1,7 @@")
            .unwrap();
        assert_eq!((builder.old_start, builder.old_count), (1, 5));
        assert_eq!((builder.new_start, builder.new_count), (1, 7));

        let builder = HunkBuilder::new(HunkId(0))
            .with_header("@@ -1 +1,2 @@")
            .unwrap();
        assert_eq!((builder.old_start, builder.old_count), (1, 1));
        assert_eq!((builder.new_start, builder.new_count), (1, 2));

        let builder = HunkBuilder::new(HunkId(0))
            .with_header("@@ -10,20 +15,25 @@ fn foo()")
            .unwrap();
        assert_eq!((builder.old_start, builder.old_count), (10, 20));
        assert_eq!((builder.new_start, builder.new_count), (15, 25));
    }
}
