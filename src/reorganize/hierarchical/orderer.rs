//! GlobalOrderer - determines commit order from dependencies

use std::collections::{HashMap, HashSet, VecDeque};

use super::types::{
    AnalysisResults, ChangeCategory, ClusterCommit, ClusterId, HierarchicalError,
};

/// Orders commits based on dependencies and logical ordering rules
pub struct GlobalOrderer;

impl GlobalOrderer {
    /// Order commits based on dependencies and heuristics
    pub fn order(
        commits: Vec<ClusterCommit>,
        analysis: &AnalysisResults,
    ) -> Result<Vec<ClusterCommit>, HierarchicalError> {
        if commits.is_empty() {
            return Ok(commits);
        }

        // Build dependency graph
        let mut graph = DependencyGraph::new(&commits);

        // Add explicit dependencies from commits
        for commit in &commits {
            for dep in &commit.depends_on {
                graph.add_edge(*dep, commit.cluster_id);
            }
        }

        // Add implicit dependencies based on categories
        Self::add_category_dependencies(&commits, analysis, &mut graph);

        // Add file-based dependencies (changes to same file should be ordered)
        Self::add_file_dependencies(&commits, analysis, &mut graph);

        // Topological sort
        let ordered_ids = graph.topological_sort()?;

        // Build ordered result
        let commit_map: HashMap<ClusterId, ClusterCommit> =
            commits.into_iter().map(|c| (c.cluster_id, c)).collect();

        let ordered: Vec<ClusterCommit> = ordered_ids
            .into_iter()
            .filter_map(|id| commit_map.get(&id).cloned())
            .collect();

        Ok(ordered)
    }

    /// Add dependencies based on change categories
    fn add_category_dependencies(
        commits: &[ClusterCommit],
        analysis: &AnalysisResults,
        graph: &mut DependencyGraph,
    ) {
        // Collect commits by category
        let mut by_category: HashMap<ChangeCategory, Vec<ClusterId>> = HashMap::new();

        for commit in commits {
            let categories: HashSet<ChangeCategory> = commit
                .hunk_ids
                .iter()
                .filter_map(|id| analysis.get(*id))
                .map(|a| a.category)
                .collect();

            for cat in categories {
                by_category.entry(cat).or_default().push(commit.cluster_id);
            }
        }

        // Apply ordering rules:
        // 1. Dependencies before features
        // 2. Features before tests
        // 3. Configuration early
        // 4. Documentation late

        let order = [
            ChangeCategory::Dependency,
            ChangeCategory::Configuration,
            ChangeCategory::Refactor,
            ChangeCategory::Feature,
            ChangeCategory::Bugfix,
            ChangeCategory::Test,
            ChangeCategory::Documentation,
            ChangeCategory::Formatting,
        ];

        for i in 0..order.len() {
            for j in (i + 1)..order.len() {
                let earlier = &order[i];
                let later = &order[j];

                if let (Some(earlier_commits), Some(later_commits)) =
                    (by_category.get(earlier), by_category.get(later))
                {
                    // Each earlier commit should come before each later commit
                    // But only add edges if they exist in the graph
                    for &earlier_id in earlier_commits {
                        for &later_id in later_commits {
                            if earlier_id != later_id {
                                graph.add_edge(earlier_id, later_id);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Add dependencies based on file changes
    fn add_file_dependencies(
        commits: &[ClusterCommit],
        analysis: &AnalysisResults,
        graph: &mut DependencyGraph,
    ) {
        // Group commits by the files they touch
        let mut commits_by_file: HashMap<String, Vec<(ClusterId, usize)>> = HashMap::new();

        for commit in commits {
            for (idx, hunk_id) in commit.hunk_ids.iter().enumerate() {
                if let Some(a) = analysis.get(*hunk_id) {
                    commits_by_file
                        .entry(a.file_path.clone())
                        .or_default()
                        .push((commit.cluster_id, idx));
                }
            }
        }

        // For each file, order commits by their hunk positions
        for (_file, mut file_commits) in commits_by_file {
            if file_commits.len() < 2 {
                continue;
            }

            // Sort by hunk index (which correlates with position in file)
            file_commits.sort_by_key(|(_, idx)| *idx);

            // Add edges between consecutive commits for the same file
            for window in file_commits.windows(2) {
                let earlier = window[0].0;
                let later = window[1].0;
                if earlier != later {
                    graph.add_edge(earlier, later);
                }
            }
        }
    }
}

/// Simple dependency graph for topological sorting
struct DependencyGraph {
    nodes: HashSet<ClusterId>,
    edges: HashMap<ClusterId, HashSet<ClusterId>>,
    reverse_edges: HashMap<ClusterId, HashSet<ClusterId>>,
}

impl DependencyGraph {
    fn new(commits: &[ClusterCommit]) -> Self {
        let nodes: HashSet<ClusterId> = commits.iter().map(|c| c.cluster_id).collect();
        let edges: HashMap<ClusterId, HashSet<ClusterId>> =
            nodes.iter().map(|&id| (id, HashSet::new())).collect();
        let reverse_edges: HashMap<ClusterId, HashSet<ClusterId>> =
            nodes.iter().map(|&id| (id, HashSet::new())).collect();

        Self {
            nodes,
            edges,
            reverse_edges,
        }
    }

    fn add_edge(&mut self, from: ClusterId, to: ClusterId) {
        if !self.nodes.contains(&from) || !self.nodes.contains(&to) {
            return;
        }
        if from == to {
            return;
        }

        self.edges.entry(from).or_default().insert(to);
        self.reverse_edges.entry(to).or_default().insert(from);
    }

    fn topological_sort(&self) -> Result<Vec<ClusterId>, HierarchicalError> {
        let mut in_degree: HashMap<ClusterId, usize> = self
            .nodes
            .iter()
            .map(|&id| {
                let degree = self.reverse_edges.get(&id).map(|s| s.len()).unwrap_or(0);
                (id, degree)
            })
            .collect();

        // Start with nodes that have no incoming edges
        let mut queue: VecDeque<ClusterId> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut result = Vec::new();

        while let Some(node) = queue.pop_front() {
            result.push(node);

            // Decrease in-degree of neighbors
            if let Some(neighbors) = self.edges.get(&node) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(&neighbor) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
        }

        if result.len() != self.nodes.len() {
            return Err(HierarchicalError::CyclicDependency);
        }

        Ok(result)
    }
}

/// Heuristic orderer that doesn't use complex dependency analysis
pub struct HeuristicOrderer;

impl HeuristicOrderer {
    pub fn order(
        mut commits: Vec<ClusterCommit>,
        analysis: &AnalysisResults,
    ) -> Vec<ClusterCommit> {
        // Simple ordering: by category priority, then by cluster ID

        let category_priority = |commit: &ClusterCommit| -> u8 {
            let categories: HashSet<ChangeCategory> = commit
                .hunk_ids
                .iter()
                .filter_map(|id| analysis.get(*id))
                .map(|a| a.category)
                .collect();

            // Lower is higher priority (comes first)
            if categories.contains(&ChangeCategory::Dependency) {
                return 0;
            }
            if categories.contains(&ChangeCategory::Configuration) {
                return 1;
            }
            if categories.contains(&ChangeCategory::Refactor) {
                return 2;
            }
            if categories.contains(&ChangeCategory::Feature) {
                return 3;
            }
            if categories.contains(&ChangeCategory::Bugfix) {
                return 4;
            }
            if categories.contains(&ChangeCategory::Test) {
                return 5;
            }
            if categories.contains(&ChangeCategory::Documentation) {
                return 6;
            }
            7
        };

        commits.sort_by(|a, b| {
            let priority_a = category_priority(a);
            let priority_b = category_priority(b);
            priority_a
                .cmp(&priority_b)
                .then_with(|| a.cluster_id.0.cmp(&b.cluster_id.0))
        });

        commits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::HunkAnalysis;
    use crate::models::HunkId;

    fn make_commit(id: usize, hunk_ids: Vec<usize>, depends_on: Vec<usize>) -> ClusterCommit {
        ClusterCommit {
            cluster_id: ClusterId(id),
            short_message: format!("Commit {}", id),
            long_message: format!("Long message for commit {}", id),
            hunk_ids: hunk_ids.into_iter().map(HunkId).collect(),
            depends_on: depends_on.into_iter().map(ClusterId).collect(),
        }
    }

    fn make_analysis_for(hunk_id: usize, category: ChangeCategory) -> HunkAnalysis {
        HunkAnalysis {
            hunk_id,
            category,
            semantic_units: vec!["test".to_string()],
            topic: "test".to_string(),
            depends_on_context: None,
            file_path: "test.rs".to_string(),
        }
    }

    #[test]
    fn test_simple_ordering() {
        let commits = vec![
            make_commit(0, vec![0], vec![]),
            make_commit(1, vec![1], vec![0]),
            make_commit(2, vec![2], vec![1]),
        ];

        let analysis = AnalysisResults::new();

        let ordered = GlobalOrderer::order(commits, &analysis).unwrap();

        // Should respect explicit dependencies: 0 -> 1 -> 2
        let ids: Vec<usize> = ordered.iter().map(|c| c.cluster_id.0).collect();
        assert_eq!(ids, vec![0, 1, 2]);
    }

    #[test]
    fn test_category_ordering() {
        let commits = vec![
            make_commit(0, vec![0], vec![]), // Feature
            make_commit(1, vec![1], vec![]), // Test
            make_commit(2, vec![2], vec![]), // Dependency
        ];

        let mut analysis = AnalysisResults::new();
        analysis.add(make_analysis_for(0, ChangeCategory::Feature));
        analysis.add(make_analysis_for(1, ChangeCategory::Test));
        analysis.add(make_analysis_for(2, ChangeCategory::Dependency));

        let ordered = HeuristicOrderer::order(commits, &analysis);

        let ids: Vec<usize> = ordered.iter().map(|c| c.cluster_id.0).collect();
        // Dependency (2) should come first, then Feature (0), then Test (1)
        assert_eq!(ids, vec![2, 0, 1]);
    }

    #[test]
    fn test_cyclic_dependency_detection() {
        let commits = vec![
            make_commit(0, vec![0], vec![1]),
            make_commit(1, vec![1], vec![0]),
        ];

        let analysis = AnalysisResults::new();

        let result = GlobalOrderer::order(commits, &analysis);

        assert!(matches!(result, Err(HierarchicalError::CyclicDependency)));
    }

    #[test]
    fn test_empty_commits() {
        let commits: Vec<ClusterCommit> = vec![];
        let analysis = AnalysisResults::new();

        let ordered = GlobalOrderer::order(commits, &analysis).unwrap();

        assert!(ordered.is_empty());
    }

    #[test]
    fn test_independent_commits() {
        let commits = vec![
            make_commit(0, vec![0], vec![]),
            make_commit(1, vec![1], vec![]),
            make_commit(2, vec![2], vec![]),
        ];

        let analysis = AnalysisResults::new();

        let ordered = GlobalOrderer::order(commits, &analysis).unwrap();

        // Should maintain some order (all have same priority)
        assert_eq!(ordered.len(), 3);
    }
}
