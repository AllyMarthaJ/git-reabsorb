//! HunkAnalyzer - parallel per-hunk semantic analysis

use std::sync::{Arc, Mutex};
use std::thread;

use log::debug;

use crate::llm::LlmClient;
use crate::models::{Hunk, HunkId, SourceCommit};
use crate::utils::{extract_json_str, format_diff_lines};

use super::types::{AnalysisResults, HierarchicalError, HunkAnalysis, HunkAnalysisResponse};

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
    pub fn analyze(
        &self,
        hunks: &[Hunk],
        source_commits: &[SourceCommit],
    ) -> Result<AnalysisResults, HierarchicalError> {
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
                    let prompt = build_analysis_prompt(hunk, source_commits);

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
    pub fn analyze_one(
        &self,
        hunk: &Hunk,
        source_commits: &[SourceCommit],
    ) -> Result<HunkAnalysis, HierarchicalError> {
        let prompt = build_analysis_prompt(hunk, source_commits);
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
    const MAX_RETRIES: u32 = 5;
    let mut last_error = String::new();

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            debug!(
                "Retrying hunk {} (attempt {}/{}): {}",
                hunk_id.0,
                attempt + 1,
                MAX_RETRIES,
                last_error
            );
            // Exponential backoff: 100ms, 200ms, 400ms
            std::thread::sleep(std::time::Duration::from_millis(100 * (1 << attempt)));
        }

        let response = match client.complete(prompt) {
            Ok(r) => r,
            Err(e) => {
                last_error = format!("LLM error: {}", e);
                continue;
            }
        };

        let parsed: HunkAnalysisResponse = match parse_analysis_response(&response) {
            Ok(p) => p,
            Err(e) => {
                last_error = format!("Parse error: {}", e);
                continue;
            }
        };

        // Successfully parsed - return the result
        return Ok(HunkAnalysis {
            hunk_id: hunk_id.0,
            category: parsed.category,
            semantic_units: parsed.semantic_units,
            topic: normalize_topic(&parsed.suggested_topic),
            depends_on_context: parsed.depends_on_context,
            file_path: file_path.to_string(),
        });
    }

    // All retries exhausted
    Err(last_error)
}

fn build_analysis_prompt(hunk: &Hunk, source_commits: &[SourceCommit]) -> String {
    let diff_content = format_diff_lines(&hunk.lines);
    let file_path = hunk.file_path.to_string_lossy();

    // Look up the original commit message to provide context about WHY this change was made
    let commit_context = hunk
        .likely_source_commits
        .first()
        .and_then(|sha| {
            source_commits
                .iter()
                .find(|c| c.sha.starts_with(sha) || sha.starts_with(&c.sha))
        })
        .map(|c| format!("\nOriginal commit: {}\n", c.message.long))
        .unwrap_or_default();

    format!(
        r#"Analyze this code change and extract structured metadata.

File: {}
Location: lines {}-{}
{}
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
        commit_context,
        diff_content
    )
}

fn parse_analysis_response(response: &str) -> Result<HunkAnalysisResponse, String> {
    let json_str = extract_json_str(response).ok_or_else(|| {
        format!(
            "No JSON found in response: {}",
            &response[..200.min(response.len())]
        )
    })?;

    serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {} in: {}", e, json_str))
}

fn normalize_topic(topic: &str) -> String {
    topic
        .to_lowercase()
        .replace([' ', '-'], "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}
