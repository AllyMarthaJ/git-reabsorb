//! CommitPlanner - generates commit messages and finalizes commit structure

use std::sync::{Arc, Mutex};
use std::thread;

use crate::models::{Hunk, HunkId};
use crate::reorganize::llm::LlmClient;

use super::types::{
    AnalysisResults, Cluster, ClusterCommit, ClusterId, CommitPlanResponse, HierarchicalError,
};

/// Plans commits from clusters
pub struct CommitPlanner {
    client: Option<Arc<dyn LlmClient + Send + Sync>>,
    max_parallel: usize,
}

impl CommitPlanner {
    pub fn new(client: Option<Arc<dyn LlmClient + Send + Sync>>) -> Self {
        Self {
            client,
            max_parallel: 4,
        }
    }

    pub fn with_parallelism(mut self, max_parallel: usize) -> Self {
        self.max_parallel = max_parallel;
        self
    }

    /// Plan commits from clusters
    pub fn plan(
        &self,
        clusters: &[Cluster],
        hunks: &[Hunk],
        analysis: &AnalysisResults,
    ) -> Result<Vec<ClusterCommit>, HierarchicalError> {
        if clusters.is_empty() {
            return Ok(Vec::new());
        }

        let client = self.client.as_ref().ok_or_else(|| {
            HierarchicalError::LlmError("LLM client is required for planning".to_string())
        })?;

        self.plan_with_llm(clusters, hunks, analysis, client)
    }

    /// Plan commits using LLM
    fn plan_with_llm(
        &self,
        clusters: &[Cluster],
        hunks: &[Hunk],
        analysis: &AnalysisResults,
        client: &Arc<dyn LlmClient + Send + Sync>,
    ) -> Result<Vec<ClusterCommit>, HierarchicalError> {
        let results = Arc::new(Mutex::new(Vec::new()));
        let errors = Arc::new(Mutex::new(Vec::new()));

        // Process clusters in parallel batches
        let chunks: Vec<_> = clusters.chunks(self.max_parallel).collect();

        for chunk in chunks {
            let handles: Vec<_> = chunk
                .iter()
                .map(|cluster| {
                    let client = Arc::clone(client);
                    let results = Arc::clone(&results);
                    let errors = Arc::clone(&errors);
                    let cluster = cluster.clone();

                    // Build context for this cluster - clone hunks to avoid lifetime issues
                    let cluster_hunks: Vec<Hunk> = cluster
                        .hunk_ids
                        .iter()
                        .filter_map(|id| hunks.iter().find(|h| h.id == *id).cloned())
                        .collect();

                    let cluster_analysis: Vec<_> = cluster
                        .hunk_ids
                        .iter()
                        .filter_map(|id| analysis.get(*id))
                        .cloned()
                        .collect();

                    thread::spawn(move || {
                        let hunk_refs: Vec<&Hunk> = cluster_hunks.iter().collect();
                        match plan_single_cluster(&client, &cluster, &hunk_refs, &cluster_analysis)
                        {
                            Ok(commits) => {
                                let mut results = results.lock().unwrap();
                                results.extend(commits);
                            }
                            Err(e) => {
                                let mut errors = errors.lock().unwrap();
                                errors.push((cluster.id, e));
                            }
                        }
                    })
                })
                .collect();

            for handle in handles {
                let _ = handle.join();
            }
        }

        // Check for errors
        let errors = Arc::try_unwrap(errors).unwrap().into_inner().unwrap();
        if !errors.is_empty() {
            let (cluster_id, error) = errors.into_iter().next().unwrap();
            return Err(HierarchicalError::PlanningFailed(cluster_id, error));
        }

        let mut commits = Arc::try_unwrap(results).unwrap().into_inner().unwrap();

        // Sort commits by cluster ID for deterministic ordering
        commits.sort_by_key(|c| c.cluster_id.0);

        Ok(commits)
    }
}

fn plan_single_cluster(
    client: &Arc<dyn LlmClient + Send + Sync>,
    cluster: &Cluster,
    hunks: &[&Hunk],
    analysis: &[super::types::HunkAnalysis],
) -> Result<Vec<ClusterCommit>, String> {
    let prompt = build_commit_prompt(cluster, hunks, analysis);

    const MAX_RETRIES: u32 = 3;
    let mut last_error = String::new();

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            eprintln!(
                "  Retrying cluster {} (attempt {}/{}): {}",
                cluster.id.0,
                attempt + 1,
                MAX_RETRIES,
                last_error
            );
            // Exponential backoff: 100ms, 200ms, 400ms
            std::thread::sleep(std::time::Duration::from_millis(100 * (1 << attempt)));
        }

        let response = match client.complete(&prompt) {
            Ok(r) => r,
            Err(e) => {
                last_error = format!("LLM error: {}", e);
                continue;
            }
        };

        let plan = match parse_commit_response(&response) {
            Ok(p) => p,
            Err(e) => {
                last_error = format!("Failed to parse commit plan: {}", e);
                continue;
            }
        };

        // Successfully parsed - return the result
        if plan.should_split && plan.split_groups.is_some() {
            // Split into multiple commits
            let groups = plan.split_groups.unwrap();
            return Ok(groups
                .into_iter()
                .enumerate()
                .map(|(i, group)| ClusterCommit {
                    cluster_id: ClusterId(cluster.id.0 * 1000 + i), // Sub-cluster ID
                    short_message: group.short_message,
                    long_message: group.long_message,
                    hunk_ids: group.hunk_ids.into_iter().map(HunkId).collect(),
                    depends_on: if i > 0 {
                        vec![ClusterId(cluster.id.0 * 1000 + i - 1)]
                    } else {
                        Vec::new()
                    },
                })
                .collect());
        } else {
            // Single commit for this cluster
            return Ok(vec![ClusterCommit {
                cluster_id: cluster.id,
                short_message: plan.short_message,
                long_message: plan.long_message,
                hunk_ids: cluster.hunk_ids.clone(),
                depends_on: Vec::new(),
            }]);
        }
    }

    // All retries exhausted
    Err(last_error)
}

fn build_commit_prompt(
    cluster: &Cluster,
    hunks: &[&Hunk],
    analysis: &[super::types::HunkAnalysis],
) -> String {
    let mut prompt = String::from(
        r#"Write a commit message for these code changes.

"#,
    );

    prompt.push_str(&format!("Topic: {}\n", cluster.topic));
    prompt.push_str(&format!(
        "Categories: {}\n\n",
        cluster
            .categories
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    prompt.push_str("Changes:\n");
    for (hunk, analysis) in hunks.iter().zip(analysis.iter()) {
        prompt.push_str(&format!(
            "- {} ({}): {}\n",
            analysis.file_path,
            analysis.category,
            analysis.semantic_units.join(", ")
        ));

        // Include abbreviated diff
        let diff_preview: String = hunk
            .lines
            .iter()
            .take(10)
            .map(|l| match l {
                crate::models::DiffLine::Context(s) => format!(" {}", s),
                crate::models::DiffLine::Added(s) => format!("+{}", s),
                crate::models::DiffLine::Removed(s) => format!("-{}", s),
            })
            .collect::<Vec<_>>()
            .join("\n");

        prompt.push_str(&format!("```diff\n{}\n```\n", diff_preview));
    }

    prompt.push_str(
        r#"
Guidelines:
- Short message: 50 chars or less, imperative mood, explains WHY not just WHAT
- Long message: Explains the motivation and context, not just a list of changes
- If changes are unrelated, set should_split=true and provide split_groups

Respond with ONLY JSON:
{
  "short_message": "Short commit message",
  "long_message": "Longer explanation of why this change was made",
  "should_split": false,
  "split_groups": null
}

Or if splitting:
{
  "short_message": "",
  "long_message": "",
  "should_split": true,
  "split_groups": [
    {"hunk_ids": [0, 1], "short_message": "First commit", "long_message": "Details"},
    {"hunk_ids": [2], "short_message": "Second commit", "long_message": "Details"}
  ]
}"#,
    );

    prompt
}

fn parse_commit_response(response: &str) -> Result<CommitPlanResponse, String> {
    let json_str = if let Some(start) = response.find('{') {
        let end = response.rfind('}').unwrap_or(response.len());
        &response[start..=end]
    } else {
        return Err("No JSON found in commit response".to_string());
    };

    serde_json::from_str(json_str).map_err(|e| format!("Failed to parse commit plan: {}", e))
}
