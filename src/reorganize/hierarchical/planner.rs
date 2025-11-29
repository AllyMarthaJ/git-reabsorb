//! CommitPlanner - generates commit messages and finalizes commit structure

use std::sync::{Arc, Mutex};
use std::thread;

use crate::models::{Hunk, HunkId};
use crate::reorganize::llm::LlmClient;

use super::types::{
    AnalysisResults, ChangeCategory, Cluster, ClusterCommit, ClusterId, CommitPlanResponse,
    HierarchicalError,
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

        match &self.client {
            Some(client) => self.plan_with_llm(clusters, hunks, analysis, client),
            None => Ok(self.plan_heuristic(clusters, hunks, analysis)),
        }
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

    /// Plan commits using heuristics (no LLM)
    fn plan_heuristic(
        &self,
        clusters: &[Cluster],
        _hunks: &[Hunk],
        analysis: &AnalysisResults,
    ) -> Vec<ClusterCommit> {
        clusters
            .iter()
            .map(|cluster| {
                let (short_msg, long_msg) = generate_heuristic_message(cluster, analysis);

                ClusterCommit {
                    cluster_id: cluster.id,
                    short_message: short_msg,
                    long_message: long_msg,
                    hunk_ids: cluster.hunk_ids.clone(),
                    depends_on: Vec::new(),
                }
            })
            .collect()
    }
}

fn plan_single_cluster(
    client: &Arc<dyn LlmClient + Send + Sync>,
    cluster: &Cluster,
    hunks: &[&Hunk],
    analysis: &[super::types::HunkAnalysis],
) -> Result<Vec<ClusterCommit>, String> {
    let prompt = build_commit_prompt(cluster, hunks, analysis);

    let response = client
        .complete(&prompt)
        .map_err(|e| format!("LLM error: {}", e))?;

    let plan = parse_commit_response(&response)?;

    if plan.should_split && plan.split_groups.is_some() {
        // Split into multiple commits
        let groups = plan.split_groups.unwrap();
        Ok(groups
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
            .collect())
    } else {
        // Single commit for this cluster
        Ok(vec![ClusterCommit {
            cluster_id: cluster.id,
            short_message: plan.short_message,
            long_message: plan.long_message,
            hunk_ids: cluster.hunk_ids.clone(),
            depends_on: Vec::new(),
        }])
    }
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

fn generate_heuristic_message(cluster: &Cluster, analysis: &AnalysisResults) -> (String, String) {
    // Collect semantic units from all hunks in the cluster
    let semantic_units: Vec<&str> = cluster
        .hunk_ids
        .iter()
        .filter_map(|id| analysis.get(*id))
        .flat_map(|a| a.semantic_units.iter().map(|s| s.as_str()))
        .collect();

    // Collect unique files
    let files: Vec<&str> = cluster
        .hunk_ids
        .iter()
        .filter_map(|id| analysis.get(*id))
        .map(|a| a.file_path.as_str())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    // Determine primary category
    let primary_category = cluster
        .categories
        .iter()
        .next()
        .copied()
        .unwrap_or(ChangeCategory::Other);

    // Generate short message based on category and topic
    let short_message = match primary_category {
        ChangeCategory::Feature => format!("Add {} functionality", cluster.topic),
        ChangeCategory::Bugfix => format!("Fix {} issue", cluster.topic),
        ChangeCategory::Refactor => format!("Refactor {}", cluster.topic),
        ChangeCategory::Test => format!("Add tests for {}", cluster.topic),
        ChangeCategory::Documentation => format!("Update {} documentation", cluster.topic),
        ChangeCategory::Configuration => format!("Update {} configuration", cluster.topic),
        ChangeCategory::Dependency => format!("Update {} dependencies", cluster.topic),
        ChangeCategory::Formatting => format!("Format {} code", cluster.topic),
        ChangeCategory::Other => format!("Update {}", cluster.topic),
    };

    // Generate long message
    let mut long_message = short_message.clone();
    long_message.push_str("\n\n");

    if !semantic_units.is_empty() {
        long_message.push_str("Changes:\n");
        for unit in semantic_units.iter().take(10) {
            long_message.push_str(&format!("- {}\n", unit));
        }
        if semantic_units.len() > 10 {
            long_message.push_str(&format!("- ... and {} more\n", semantic_units.len() - 10));
        }
    }

    if files.len() > 1 {
        long_message.push_str(&format!("\nAffected files: {}\n", files.len()));
    }

    (short_message, long_message)
}

/// Heuristic planner that doesn't use LLM
pub struct HeuristicPlanner;

impl HeuristicPlanner {
    pub fn plan(clusters: &[Cluster], analysis: &AnalysisResults) -> Vec<ClusterCommit> {
        clusters
            .iter()
            .map(|cluster| {
                let (short_msg, long_msg) = generate_heuristic_message(cluster, analysis);

                ClusterCommit {
                    cluster_id: cluster.id,
                    short_message: short_msg,
                    long_message: long_msg,
                    hunk_ids: cluster.hunk_ids.clone(),
                    depends_on: Vec::new(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::ClusterFormationReason;
    use super::*;
    use std::collections::HashSet;

    fn make_test_cluster(id: usize, topic: &str, hunk_ids: Vec<usize>) -> Cluster {
        Cluster {
            id: ClusterId(id),
            hunk_ids: hunk_ids.into_iter().map(HunkId).collect(),
            topic: topic.to_string(),
            categories: HashSet::from([ChangeCategory::Feature]),
            formation_reason: ClusterFormationReason::SameTopic(topic.to_string()),
        }
    }

    #[test]
    fn test_heuristic_message_feature() {
        let cluster = make_test_cluster(0, "authentication", vec![0, 1]);

        let mut analysis = AnalysisResults::new();
        analysis.add(super::super::types::HunkAnalysis {
            hunk_id: 0,
            category: ChangeCategory::Feature,
            semantic_units: vec!["add login function".to_string()],
            topic: "authentication".to_string(),
            depends_on_context: None,
            file_path: "src/auth.rs".to_string(),
        });
        analysis.add(super::super::types::HunkAnalysis {
            hunk_id: 1,
            category: ChangeCategory::Feature,
            semantic_units: vec!["add logout function".to_string()],
            topic: "authentication".to_string(),
            depends_on_context: None,
            file_path: "src/auth.rs".to_string(),
        });

        let (short, long) = generate_heuristic_message(&cluster, &analysis);

        assert!(short.contains("authentication"));
        assert!(long.contains("login"));
        assert!(long.contains("logout"));
    }

    #[test]
    fn test_heuristic_message_bugfix() {
        let mut cluster = make_test_cluster(0, "validation", vec![0]);
        cluster.categories = HashSet::from([ChangeCategory::Bugfix]);

        let mut analysis = AnalysisResults::new();
        analysis.add(super::super::types::HunkAnalysis {
            hunk_id: 0,
            category: ChangeCategory::Bugfix,
            semantic_units: vec!["fix null check".to_string()],
            topic: "validation".to_string(),
            depends_on_context: None,
            file_path: "src/validate.rs".to_string(),
        });

        let (short, _) = generate_heuristic_message(&cluster, &analysis);

        assert!(short.contains("Fix"));
    }

    #[test]
    fn test_heuristic_planner() {
        let clusters = vec![
            make_test_cluster(0, "auth", vec![0, 1]),
            make_test_cluster(1, "api", vec![2]),
        ];

        let analysis = AnalysisResults::new();

        let commits = HeuristicPlanner::plan(&clusters, &analysis);

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].cluster_id.0, 0);
        assert_eq!(commits[1].cluster_id.0, 1);
    }
}
