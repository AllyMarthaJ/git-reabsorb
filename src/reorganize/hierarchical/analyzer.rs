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
        file_path,
        hunk.old_start,
        hunk.old_start + hunk.old_count,
        diff_content
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
        .replace([' ', '-'], "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

