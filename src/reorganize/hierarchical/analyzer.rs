//! HunkAnalyzer - parallel per-hunk semantic analysis

use std::sync::{Arc, Mutex};
use std::thread;

use crate::models::{DiffLine, Hunk, HunkId};
use crate::reorganize::llm::LlmClient;

use super::types::{
    AnalysisResults, ChangeCategory, HierarchicalError, HunkAnalysis, HunkAnalysisResponse,
};

/// Analyzes hunks to extract semantic metadata
pub struct HunkAnalyzer {
    client: Arc<dyn LlmClient + Send + Sync>,
    max_parallel: usize,
}

impl HunkAnalyzer {
    pub fn new(client: Arc<dyn LlmClient + Send + Sync>) -> Self {
        Self {
            client,
            max_parallel: 8, // Default parallelism
        }
    }

    pub fn with_parallelism(mut self, max_parallel: usize) -> Self {
        self.max_parallel = max_parallel;
        self
    }

    /// Analyze all hunks in parallel
    pub fn analyze(&self, hunks: &[Hunk]) -> Result<AnalysisResults, HierarchicalError> {
        if hunks.is_empty() {
            return Ok(AnalysisResults::new());
        }

        let results = Arc::new(Mutex::new(AnalysisResults::new()));
        let errors = Arc::new(Mutex::new(Vec::new()));

        // Process hunks in batches to limit parallelism
        let chunks: Vec<_> = hunks.chunks(self.max_parallel).collect();

        for chunk in chunks {
            let handles: Vec<_> = chunk
                .iter()
                .map(|hunk| {
                    let client = Arc::clone(&self.client);
                    let results = Arc::clone(&results);
                    let errors = Arc::clone(&errors);
                    let hunk_id = hunk.id;
                    let file_path = hunk.file_path.to_string_lossy().to_string();
                    let prompt = build_analysis_prompt(hunk);

                    thread::spawn(move || {
                        match analyze_single_hunk(&client, hunk_id, &file_path, &prompt) {
                            Ok(analysis) => {
                                let mut results = results.lock().unwrap();
                                results.add(analysis);
                            }
                            Err(e) => {
                                let mut errors = errors.lock().unwrap();
                                errors.push((hunk_id, e));
                            }
                        }
                    })
                })
                .collect();

            // Wait for this batch to complete
            for handle in handles {
                let _ = handle.join();
            }
        }

        // Check for errors
        let errors = Arc::try_unwrap(errors).unwrap().into_inner().unwrap();
        if !errors.is_empty() {
            let (hunk_id, error) = errors.into_iter().next().unwrap();
            return Err(HierarchicalError::AnalysisFailed(hunk_id.0, error));
        }

        Ok(Arc::try_unwrap(results).unwrap().into_inner().unwrap())
    }

    /// Analyze a single hunk (for testing or sequential processing)
    pub fn analyze_one(&self, hunk: &Hunk) -> Result<HunkAnalysis, HierarchicalError> {
        let prompt = build_analysis_prompt(hunk);
        let file_path = hunk.file_path.to_string_lossy().to_string();
        analyze_single_hunk(&self.client, hunk.id, &file_path, &prompt)
            .map_err(|e| HierarchicalError::AnalysisFailed(hunk.id.0, e))
    }
}

fn analyze_single_hunk(
    client: &Arc<dyn LlmClient + Send + Sync>,
    hunk_id: HunkId,
    file_path: &str,
    prompt: &str,
) -> Result<HunkAnalysis, String> {
    let response = client
        .complete(prompt)
        .map_err(|e| format!("LLM error: {}", e))?;

    let parsed: HunkAnalysisResponse =
        parse_analysis_response(&response).map_err(|e| format!("Parse error: {}", e))?;

    Ok(HunkAnalysis {
        hunk_id: hunk_id.0,
        category: parsed.category,
        semantic_units: parsed.semantic_units,
        topic: normalize_topic(&parsed.suggested_topic),
        depends_on_context: parsed.depends_on_context,
        file_path: file_path.to_string(),
    })
}

fn build_analysis_prompt(hunk: &Hunk) -> String {
    let diff_content = format_diff_lines(&hunk.lines);
    let file_path = hunk.file_path.to_string_lossy();

    format!(
        r#"Analyze this code change and extract structured metadata.

File: {}
Location: lines {}-{}

```diff
{}
```

Respond with ONLY a JSON object (no markdown, no explanation):
{{
  "category": "feature|bugfix|refactor|test|documentation|configuration|dependency|formatting|other",
  "semantic_units": ["brief description of each logical change in this diff"],
  "suggested_topic": "single_word_or_short_phrase for grouping related changes",
  "depends_on_context": "what must exist for this change to work (or null if standalone)"
}}

Guidelines:
- category: Choose the primary purpose of this change
- semantic_units: Be specific (e.g., "add validate_token function" not just "add function")
- suggested_topic: Use lowercase with underscores, be consistent (e.g., "authentication", "error_handling", "user_api")
- depends_on_context: Mention imports, types, or functions this change relies on"#,
        file_path, hunk.old_start, hunk.old_start + hunk.old_count, diff_content
    )
}

fn format_diff_lines(lines: &[DiffLine]) -> String {
    lines
        .iter()
        .map(|line| match line {
            DiffLine::Context(s) => format!(" {}", s),
            DiffLine::Added(s) => format!("+{}", s),
            DiffLine::Removed(s) => format!("-{}", s),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_analysis_response(response: &str) -> Result<HunkAnalysisResponse, String> {
    // Try to extract JSON from the response
    let json_str = extract_json(response)?;

    serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {} in: {}", e, json_str))
}

fn extract_json(response: &str) -> Result<&str, String> {
    // Try to find JSON in code fence
    if let Some(start) = response.find("```json") {
        let content_start = start + 7;
        let end = response[content_start..]
            .find("```")
            .map(|e| content_start + e)
            .unwrap_or(response.len());
        return Ok(response[content_start..end].trim());
    }

    // Try to find JSON in generic code fence
    if let Some(start) = response.find("```") {
        let content_start = start + 3;
        let line_end = response[content_start..]
            .find('\n')
            .map(|n| content_start + n + 1)
            .unwrap_or(content_start);
        let end = response[line_end..]
            .find("```")
            .map(|e| line_end + e)
            .unwrap_or(response.len());
        return Ok(response[line_end..end].trim());
    }

    // Try to find raw JSON
    if let Some(start) = response.find('{') {
        if let Some(end) = response.rfind('}') {
            return Ok(response[start..=end].trim());
        }
    }

    Err(format!(
        "No JSON found in response: {}",
        &response[..200.min(response.len())]
    ))
}

fn normalize_topic(topic: &str) -> String {
    topic
        .to_lowercase()
        .replace(' ', "_")
        .replace('-', "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Fallback analyzer that doesn't use LLM - uses heuristics instead
pub struct HeuristicAnalyzer;

impl HeuristicAnalyzer {
    pub fn analyze(hunks: &[Hunk]) -> AnalysisResults {
        let mut results = AnalysisResults::new();

        for hunk in hunks {
            let analysis = Self::analyze_one(hunk);
            results.add(analysis);
        }

        results
    }

    pub fn analyze_one(hunk: &Hunk) -> HunkAnalysis {
        let file_path = hunk.file_path.to_string_lossy().to_string();
        let category = Self::infer_category(&file_path, &hunk.lines);
        let topic = Self::infer_topic(&file_path);
        let semantic_units = Self::infer_semantic_units(&hunk.lines);

        HunkAnalysis {
            hunk_id: hunk.id.0,
            category,
            semantic_units,
            topic,
            depends_on_context: None,
            file_path,
        }
    }

    fn infer_category(file_path: &str, lines: &[DiffLine]) -> ChangeCategory {
        let path_lower = file_path.to_lowercase();

        // Check file path patterns
        if path_lower.contains("test") || path_lower.contains("spec") {
            return ChangeCategory::Test;
        }
        if path_lower.ends_with(".md")
            || path_lower.contains("readme")
            || path_lower.contains("doc")
        {
            return ChangeCategory::Documentation;
        }
        if path_lower.contains("config")
            || path_lower.ends_with(".toml")
            || path_lower.ends_with(".yaml")
            || path_lower.ends_with(".yml")
            || path_lower.ends_with(".json")
        {
            return ChangeCategory::Configuration;
        }
        if path_lower.contains("cargo.toml")
            || path_lower.contains("package.json")
            || path_lower.contains("requirements")
        {
            return ChangeCategory::Dependency;
        }

        // Check content patterns
        let content: String = lines
            .iter()
            .filter_map(|l| match l {
                DiffLine::Added(s) | DiffLine::Removed(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        let content_lower = content.to_lowercase();

        if content_lower.contains("fix") || content_lower.contains("bug") {
            return ChangeCategory::Bugfix;
        }
        if content_lower.contains("refactor") || content_lower.contains("rename") {
            return ChangeCategory::Refactor;
        }

        // Check if it's mostly whitespace/formatting changes
        let significant_changes = lines.iter().filter(|l| {
            matches!(l, DiffLine::Added(s) | DiffLine::Removed(s) if !s.trim().is_empty())
        }).count();

        if significant_changes == 0 {
            return ChangeCategory::Formatting;
        }

        // Default to feature
        ChangeCategory::Feature
    }

    fn infer_topic(file_path: &str) -> String {
        let path = std::path::Path::new(file_path);

        // Use parent directory as topic if available
        if let Some(parent) = path.parent() {
            if let Some(dir_name) = parent.file_name() {
                let dir = dir_name.to_string_lossy().to_lowercase();
                if !dir.is_empty() && dir != "src" && dir != "lib" {
                    return dir.replace('-', "_");
                }
            }
        }

        // Use file stem
        if let Some(stem) = path.file_stem() {
            return stem.to_string_lossy().to_lowercase().replace('-', "_");
        }

        "general".to_string()
    }

    fn infer_semantic_units(lines: &[DiffLine]) -> Vec<String> {
        let mut units = Vec::new();

        for line in lines {
            if let DiffLine::Added(content) = line {
                let trimmed = content.trim();

                // Detect function definitions
                if trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("async fn ")
                    || trimmed.starts_with("pub async fn ")
                {
                    if let Some(name) = extract_function_name(trimmed) {
                        units.push(format!("add function {}", name));
                    }
                }
                // Detect struct definitions
                else if trimmed.starts_with("struct ")
                    || trimmed.starts_with("pub struct ")
                {
                    if let Some(name) = extract_struct_name(trimmed) {
                        units.push(format!("add struct {}", name));
                    }
                }
                // Detect impl blocks
                else if trimmed.starts_with("impl ") {
                    if let Some(name) = extract_impl_name(trimmed) {
                        units.push(format!("add impl for {}", name));
                    }
                }
                // Detect use statements
                else if trimmed.starts_with("use ") {
                    units.push("add import".to_string());
                }
            }
        }

        if units.is_empty() {
            units.push("modify code".to_string());
        }

        units
    }
}

fn extract_function_name(line: &str) -> Option<String> {
    // Look for "fn name(" pattern
    let fn_idx = line.find("fn ")?;
    let after_fn = &line[fn_idx + 3..];
    let paren_idx = after_fn.find('(')?;
    let name = after_fn[..paren_idx].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn extract_struct_name(line: &str) -> Option<String> {
    // Look for "struct Name" pattern
    let struct_idx = line.find("struct ")?;
    let after_struct = &line[struct_idx + 7..];
    let end_idx = after_struct
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(after_struct.len());
    let name = &after_struct[..end_idx];
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn extract_impl_name(line: &str) -> Option<String> {
    // Look for "impl Name" or "impl Trait for Name"
    let impl_idx = line.find("impl ")?;
    let after_impl = &line[impl_idx + 5..];

    if let Some(for_idx) = after_impl.find(" for ") {
        // impl Trait for Type
        let after_for = &after_impl[for_idx + 5..];
        let end_idx = after_for
            .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '<')
            .unwrap_or(after_for.len());
        Some(after_for[..end_idx].to_string())
    } else {
        // impl Type
        let end_idx = after_impl
            .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '<')
            .unwrap_or(after_impl.len());
        Some(after_impl[..end_idx].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_test_hunk(id: usize, file: &str, lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            id: HunkId(id),
            file_path: PathBuf::from(file),
            old_start: 1,
            old_count: 5,
            new_start: 1,
            new_count: 6,
            lines,
            likely_source_commits: vec![],
        }
    }

    #[test]
    fn test_normalize_topic() {
        assert_eq!(normalize_topic("User Authentication"), "user_authentication");
        assert_eq!(normalize_topic("error-handling"), "error_handling");
        assert_eq!(normalize_topic("API Client"), "api_client");
    }

    #[test]
    fn test_heuristic_category_test_file() {
        let hunk = make_test_hunk(
            0,
            "tests/auth_test.rs",
            vec![DiffLine::Added("#[test]".to_string())],
        );
        let analysis = HeuristicAnalyzer::analyze_one(&hunk);
        assert_eq!(analysis.category, ChangeCategory::Test);
    }

    #[test]
    fn test_heuristic_category_docs() {
        let hunk = make_test_hunk(
            0,
            "README.md",
            vec![DiffLine::Added("# Title".to_string())],
        );
        let analysis = HeuristicAnalyzer::analyze_one(&hunk);
        assert_eq!(analysis.category, ChangeCategory::Documentation);
    }

    #[test]
    fn test_heuristic_topic_from_directory() {
        let hunk = make_test_hunk(
            0,
            "src/auth/login.rs",
            vec![DiffLine::Added("code".to_string())],
        );
        let analysis = HeuristicAnalyzer::analyze_one(&hunk);
        assert_eq!(analysis.topic, "auth");
    }

    #[test]
    fn test_extract_function_name() {
        assert_eq!(extract_function_name("fn main() {"), Some("main".to_string()));
        assert_eq!(
            extract_function_name("pub fn validate(x: i32) -> bool {"),
            Some("validate".to_string())
        );
        assert_eq!(
            extract_function_name("pub async fn fetch_data() {"),
            Some("fetch_data".to_string())
        );
    }

    #[test]
    fn test_extract_struct_name() {
        assert_eq!(extract_struct_name("struct User {"), Some("User".to_string()));
        assert_eq!(
            extract_struct_name("pub struct Config {"),
            Some("Config".to_string())
        );
    }

    #[test]
    fn test_heuristic_semantic_units() {
        let hunk = make_test_hunk(
            0,
            "src/lib.rs",
            vec![
                DiffLine::Added("pub fn validate_token(token: &str) -> bool {".to_string()),
                DiffLine::Added("    true".to_string()),
                DiffLine::Added("}".to_string()),
            ],
        );
        let analysis = HeuristicAnalyzer::analyze_one(&hunk);
        assert!(analysis.semantic_units.iter().any(|u| u.contains("validate_token")));
    }
}
