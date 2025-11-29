//! Clusterer - groups hunks into candidate commit clusters

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::models::{Hunk, HunkId};
use crate::reorganize::llm::LlmClient;

use super::types::{
    AnalysisResults, ChangeCategory, Cluster, ClusterFormationReason, ClusterId, HierarchicalError,
    RelationshipResponse,
};

/// Configuration for clustering behavior
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// Minimum number of hunks to trigger cross-file analysis
    pub cross_file_threshold: usize,
    /// Whether to use LLM for cross-file relationship detection
    pub use_llm_relationships: bool,
    /// Maximum hunks per cluster before suggesting split
    pub max_cluster_size: usize,
    /// Whether to group tests with their implementation
    pub group_tests_with_impl: bool,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            cross_file_threshold: 5,
            use_llm_relationships: true,
            max_cluster_size: 20,
            group_tests_with_impl: true,
        }
    }
}

/// Groups analyzed hunks into clusters for commits
pub struct Clusterer {
    client: Option<Arc<dyn LlmClient + Send + Sync>>,
    config: ClusterConfig,
}

impl Clusterer {
    pub fn new(client: Option<Arc<dyn LlmClient + Send + Sync>>) -> Self {
        Self {
            client,
            config: ClusterConfig::default(),
        }
    }

    pub fn with_config(mut self, config: ClusterConfig) -> Self {
        self.config = config;
        self
    }

    /// Cluster hunks based on analysis results
    pub fn cluster(
        &self,
        hunks: &[Hunk],
        analysis: &AnalysisResults,
    ) -> Result<Vec<Cluster>, HierarchicalError> {
        if hunks.is_empty() {
            return Ok(Vec::new());
        }

        // Step 1: Build initial clusters from topics
        let mut clusters = self.build_topic_clusters(analysis);

        // Step 2: Merge small clusters or split large ones
        clusters = self.balance_clusters(clusters, analysis);

        // Step 3: Handle cross-file relationships if LLM is available
        if self.config.use_llm_relationships
            && self.client.is_some()
            && hunks.len() >= self.config.cross_file_threshold
        {
            clusters = self.refine_with_llm(clusters, hunks, analysis)?;
        }

        // Step 4: Group tests with implementations if configured
        if self.config.group_tests_with_impl {
            clusters = self.group_tests_with_implementations(clusters, analysis);
        }

        // Step 5: Validate all hunks are assigned
        self.validate_complete_assignment(hunks, &clusters)?;

        Ok(clusters)
    }

    /// Build initial clusters based on topic grouping
    fn build_topic_clusters(&self, analysis: &AnalysisResults) -> Vec<Cluster> {
        let mut clusters = Vec::new();
        let mut next_id = 0;

        for (topic, hunk_ids) in &analysis.by_topic {
            if hunk_ids.is_empty() {
                continue;
            }

            let categories: HashSet<ChangeCategory> = hunk_ids
                .iter()
                .filter_map(|id| analysis.get(*id))
                .map(|a| a.category)
                .collect();

            clusters.push(Cluster {
                id: ClusterId(next_id),
                hunk_ids: hunk_ids.clone(),
                topic: topic.clone(),
                categories,
                formation_reason: ClusterFormationReason::SameTopic(topic.clone()),
            });

            next_id += 1;
        }

        clusters
    }

    /// Balance cluster sizes - merge small, split large
    fn balance_clusters(
        &self,
        mut clusters: Vec<Cluster>,
        analysis: &AnalysisResults,
    ) -> Vec<Cluster> {
        // Sort clusters by size for processing
        clusters.sort_by_key(|c| c.hunk_ids.len());

        // Merge very small clusters (1-2 hunks) with related larger ones
        let mut merged = Vec::new();
        let mut to_merge: Vec<Cluster> = Vec::new();

        for cluster in clusters {
            if cluster.hunk_ids.len() <= 2 {
                to_merge.push(cluster);
            } else {
                merged.push(cluster);
            }
        }

        // Try to merge small clusters into existing ones by category
        for small in to_merge {
            let mut merged_into = false;

            // Find a cluster with overlapping categories
            for existing in &mut merged {
                if !small.categories.is_disjoint(&existing.categories) {
                    existing.hunk_ids.extend(small.hunk_ids.clone());
                    existing.categories.extend(small.categories.iter().cloned());
                    merged_into = true;
                    break;
                }
            }

            if !merged_into {
                merged.push(small);
            }
        }

        // Split large clusters by file
        let mut final_clusters = Vec::new();
        let mut next_id = merged.iter().map(|c| c.id.0).max().unwrap_or(0) + 1;

        for cluster in merged {
            if cluster.hunk_ids.len() > self.config.max_cluster_size {
                // Split by file
                let by_file = self.group_by_file(&cluster.hunk_ids, analysis);

                for (file_path, hunk_ids) in by_file {
                    let categories: HashSet<_> = hunk_ids
                        .iter()
                        .filter_map(|id| analysis.get(*id))
                        .map(|a| a.category)
                        .collect();

                    final_clusters.push(Cluster {
                        id: ClusterId(next_id),
                        hunk_ids,
                        topic: cluster.topic.clone(),
                        categories,
                        formation_reason: ClusterFormationReason::SameFile(file_path),
                    });
                    next_id += 1;
                }
            } else {
                final_clusters.push(cluster);
            }
        }

        final_clusters
    }

    /// Group hunk IDs by their file path
    fn group_by_file(
        &self,
        hunk_ids: &[HunkId],
        analysis: &AnalysisResults,
    ) -> HashMap<String, Vec<HunkId>> {
        let mut by_file: HashMap<String, Vec<HunkId>> = HashMap::new();

        for &hunk_id in hunk_ids {
            if let Some(a) = analysis.get(hunk_id) {
                by_file
                    .entry(a.file_path.clone())
                    .or_default()
                    .push(hunk_id);
            }
        }

        by_file
    }

    /// Use LLM to detect cross-file relationships and refine clusters
    fn refine_with_llm(
        &self,
        mut clusters: Vec<Cluster>,
        hunks: &[Hunk],
        analysis: &AnalysisResults,
    ) -> Result<Vec<Cluster>, HierarchicalError> {
        let client = match &self.client {
            Some(c) => c,
            None => return Ok(clusters),
        };

        // Build a summary of hunks for the LLM
        let prompt = build_relationship_prompt(hunks, analysis);

        let response = client
            .complete(&prompt)
            .map_err(|e| HierarchicalError::LlmError(e.to_string()))?;

        let relationships = parse_relationship_response(&response)?;

        // Apply relationship groupings
        clusters = self.apply_relationships(clusters, relationships, analysis);

        Ok(clusters)
    }

    /// Apply LLM-detected relationships to clusters
    fn apply_relationships(
        &self,
        mut clusters: Vec<Cluster>,
        relationships: RelationshipResponse,
        analysis: &AnalysisResults,
    ) -> Vec<Cluster> {
        // Build a map of hunk_id -> current cluster index
        let mut hunk_to_cluster: HashMap<HunkId, usize> = HashMap::new();
        for (idx, cluster) in clusters.iter().enumerate() {
            for &hunk_id in &cluster.hunk_ids {
                hunk_to_cluster.insert(hunk_id, idx);
            }
        }

        // Process each relationship group
        for group in relationships.groups {
            let hunk_ids: Vec<HunkId> = group.hunk_ids.iter().map(|&id| HunkId(id)).collect();

            // Find which clusters these hunks are currently in
            let involved_clusters: HashSet<usize> = hunk_ids
                .iter()
                .filter_map(|id| hunk_to_cluster.get(id).copied())
                .collect();

            if involved_clusters.len() <= 1 {
                // All hunks already in same cluster, nothing to do
                continue;
            }

            // Merge the hunks into the first cluster
            let target_cluster = *involved_clusters.iter().min().unwrap();

            for &hunk_id in &hunk_ids {
                if let Some(&current) = hunk_to_cluster.get(&hunk_id) {
                    if current != target_cluster {
                        // Remove from current cluster
                        clusters[current].hunk_ids.retain(|&id| id != hunk_id);
                        // Add to target cluster
                        if !clusters[target_cluster].hunk_ids.contains(&hunk_id) {
                            clusters[target_cluster].hunk_ids.push(hunk_id);
                        }
                        // Update map
                        hunk_to_cluster.insert(hunk_id, target_cluster);
                    }
                }
            }

            // Update formation reason
            clusters[target_cluster].formation_reason =
                ClusterFormationReason::LlmGrouped(group.reason.clone());

            // Update categories
            for &hunk_id in &hunk_ids {
                if let Some(a) = analysis.get(hunk_id) {
                    clusters[target_cluster].categories.insert(a.category);
                }
            }
        }

        // Remove empty clusters
        clusters.retain(|c| !c.hunk_ids.is_empty());

        clusters
    }

    /// Group test hunks with their corresponding implementation hunks
    fn group_tests_with_implementations(
        &self,
        mut clusters: Vec<Cluster>,
        _analysis: &AnalysisResults,
    ) -> Vec<Cluster> {
        // Find test clusters and implementation clusters with same topic
        let test_cluster_indices: Vec<usize> = clusters
            .iter()
            .enumerate()
            .filter(|(_, c)| c.categories.contains(&ChangeCategory::Test))
            .map(|(i, _)| i)
            .collect();

        for test_idx in test_cluster_indices.into_iter().rev() {
            let test_topic = clusters[test_idx].topic.clone();

            // Find an implementation cluster with the same topic
            let impl_idx = clusters.iter().enumerate().find(|(i, c)| {
                *i != test_idx
                    && c.topic == test_topic
                    && !c.categories.contains(&ChangeCategory::Test)
            });

            if let Some((impl_idx, _)) = impl_idx {
                // Merge test hunks into implementation cluster
                let test_hunks = clusters[test_idx].hunk_ids.clone();
                clusters[impl_idx].hunk_ids.extend(test_hunks);
                clusters[impl_idx].categories.insert(ChangeCategory::Test);

                // Mark test cluster for removal
                clusters[test_idx].hunk_ids.clear();
            }
        }

        // Remove empty clusters
        clusters.retain(|c| !c.hunk_ids.is_empty());

        clusters
    }

    /// Validate that all hunks are assigned to exactly one cluster
    fn validate_complete_assignment(
        &self,
        hunks: &[Hunk],
        clusters: &[Cluster],
    ) -> Result<(), HierarchicalError> {
        let all_hunk_ids: HashSet<HunkId> = hunks.iter().map(|h| h.id).collect();
        let assigned_hunk_ids: HashSet<HunkId> = clusters
            .iter()
            .flat_map(|c| c.hunk_ids.iter().copied())
            .collect();

        let unassigned: Vec<HunkId> = all_hunk_ids
            .difference(&assigned_hunk_ids)
            .copied()
            .collect();

        if !unassigned.is_empty() {
            return Err(HierarchicalError::UnassignedHunks(unassigned));
        }

        // Check for duplicates
        let mut seen = HashSet::new();
        for cluster in clusters {
            for &hunk_id in &cluster.hunk_ids {
                if !seen.insert(hunk_id) {
                    return Err(HierarchicalError::ValidationFailed(format!(
                        "Hunk {} assigned to multiple clusters",
                        hunk_id.0
                    )));
                }
            }
        }

        Ok(())
    }
}

fn build_relationship_prompt(hunks: &[Hunk], analysis: &AnalysisResults) -> String {
    let mut prompt = String::from(
        r#"Analyze these code changes and identify which ones should be in the same commit.

Changes:
"#,
    );

    for hunk in hunks {
        if let Some(a) = analysis.get(hunk.id) {
            prompt.push_str(&format!(
                "- Hunk {} ({}): {} [{}]\n",
                hunk.id.0,
                a.file_path,
                a.semantic_units.join(", "),
                a.category
            ));
        }
    }

    prompt.push_str(
        r#"
Group hunks that should be committed together. Consider:
- Hunks that implement the same feature across multiple files
- A function and its callers
- An interface/trait and its implementations
- Code and its tests

Respond with ONLY JSON:
{
  "groups": [
    {"hunk_ids": [0, 1, 2], "reason": "implement user authentication"},
    {"hunk_ids": [3, 4], "reason": "add error handling for API"}
  ]
}

Only include groups with hunks from DIFFERENT files that belong together.
Do not include hunks that are already logically grouped."#,
    );

    prompt
}

fn parse_relationship_response(response: &str) -> Result<RelationshipResponse, HierarchicalError> {
    // Extract JSON from response
    let json_str = if let Some(start) = response.find('{') {
        let end = response.rfind('}').unwrap_or(response.len());
        &response[start..=end]
    } else {
        return Err(HierarchicalError::LlmError(
            "No JSON found in relationship response".to_string(),
        ));
    };

    serde_json::from_str(json_str)
        .map_err(|e| HierarchicalError::LlmError(format!("Failed to parse relationships: {}", e)))
}

/// Fallback clusterer that doesn't use LLM
pub struct HeuristicClusterer;

impl HeuristicClusterer {
    pub fn cluster(_hunks: &[Hunk], analysis: &AnalysisResults) -> Vec<Cluster> {
        // Simple strategy: cluster by topic, then by file
        let mut clusters = Vec::new();
        let mut next_id = 0;

        // First pass: group by topic
        for (topic, hunk_ids) in &analysis.by_topic {
            if hunk_ids.is_empty() {
                continue;
            }

            let categories: HashSet<ChangeCategory> = hunk_ids
                .iter()
                .filter_map(|id| analysis.get(*id))
                .map(|a| a.category)
                .collect();

            clusters.push(Cluster {
                id: ClusterId(next_id),
                hunk_ids: hunk_ids.clone(),
                topic: topic.clone(),
                categories,
                formation_reason: ClusterFormationReason::SameTopic(topic.clone()),
            });

            next_id += 1;
        }

        // If no topics, fall back to file-based clustering
        if clusters.is_empty() {
            for (file_path, hunk_ids) in &analysis.by_file {
                if hunk_ids.is_empty() {
                    continue;
                }

                let categories: HashSet<ChangeCategory> = hunk_ids
                    .iter()
                    .filter_map(|id| analysis.get(*id))
                    .map(|a| a.category)
                    .collect();

                // Derive topic from file path
                let topic = std::path::Path::new(file_path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "general".to_string());

                clusters.push(Cluster {
                    id: ClusterId(next_id),
                    hunk_ids: hunk_ids.clone(),
                    topic,
                    categories,
                    formation_reason: ClusterFormationReason::SameFile(file_path.clone()),
                });

                next_id += 1;
            }
        }

        clusters
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::HunkAnalysis;
    use super::*;
    use crate::models::DiffLine;
    use std::path::PathBuf;

    fn make_test_hunk(id: usize, file: &str) -> Hunk {
        Hunk {
            id: HunkId(id),
            file_path: PathBuf::from(file),
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 4,
            lines: vec![DiffLine::Added("test".to_string())],
            likely_source_commits: vec![],
        }
    }

    fn make_analysis(
        hunk_id: usize,
        file: &str,
        topic: &str,
        category: ChangeCategory,
    ) -> HunkAnalysis {
        HunkAnalysis {
            hunk_id,
            category,
            semantic_units: vec!["test change".to_string()],
            topic: topic.to_string(),
            depends_on_context: None,
            file_path: file.to_string(),
        }
    }

    #[test]
    fn test_topic_clustering() {
        let mut analysis = AnalysisResults::new();
        analysis.add(make_analysis(
            0,
            "src/auth/login.rs",
            "auth",
            ChangeCategory::Feature,
        ));
        analysis.add(make_analysis(
            1,
            "src/auth/logout.rs",
            "auth",
            ChangeCategory::Feature,
        ));
        analysis.add(make_analysis(
            2,
            "src/api/users.rs",
            "users",
            ChangeCategory::Feature,
        ));

        let clusters = HeuristicClusterer::cluster(&[], &analysis);

        assert_eq!(clusters.len(), 2);

        let auth_cluster = clusters.iter().find(|c| c.topic == "auth").unwrap();
        assert_eq!(auth_cluster.hunk_ids.len(), 2);

        let users_cluster = clusters.iter().find(|c| c.topic == "users").unwrap();
        assert_eq!(users_cluster.hunk_ids.len(), 1);
    }

    #[test]
    fn test_cluster_validation() {
        let hunks = vec![make_test_hunk(0, "a.rs"), make_test_hunk(1, "b.rs")];

        let mut analysis = AnalysisResults::new();
        analysis.add(make_analysis(0, "a.rs", "topic", ChangeCategory::Feature));
        // Note: hunk 1 is not analyzed

        let clusterer = Clusterer::new(None);

        // This should fail because hunk 1 is unassigned
        let clusters = vec![Cluster {
            id: ClusterId(0),
            hunk_ids: vec![HunkId(0)],
            topic: "topic".to_string(),
            categories: HashSet::from([ChangeCategory::Feature]),
            formation_reason: ClusterFormationReason::Fallback,
        }];

        let result = clusterer.validate_complete_assignment(&hunks, &clusters);
        assert!(matches!(result, Err(HierarchicalError::UnassignedHunks(_))));
    }
}
