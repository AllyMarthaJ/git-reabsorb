//! GlobalOrderer - determines commit order from dependencies

use std::collections::{HashMap, HashSet, VecDeque};

use log::debug;

use crate::models::{HunkId, PlannedChange, PlannedCommit, PlannedCommitId};

use super::types::{AnalysisResults, ChangeCategory, HierarchicalError};

/// Orders commits based on dependencies and logical ordering rules
pub struct GlobalOrderer;

impl GlobalOrderer {
    /// Order commits based on dependencies and heuristics
    pub fn order(
        commits: Vec<PlannedCommit>,
        analysis: &AnalysisResults,
    ) -> Result<Vec<PlannedCommit>, HierarchicalError> {
        if commits.is_empty() {
            return Ok(commits);
        }

        // Build dependency graph
        let mut graph = DependencyGraph::new(&commits);

        // Add explicit dependencies from commits
        for commit in &commits {
            for dep in &commit.depends_on {
                graph.add_edge(*dep, commit.id);
            }
        }

        // Add implicit dependencies based on categories
        Self::add_category_dependencies(&commits, analysis, &mut graph);

        // Add file-based dependencies (changes to same file should be ordered)
        Self::add_file_dependencies(&commits, analysis, &mut graph);

        // Try topological sort, breaking cycles if needed
        const MAX_CYCLE_BREAKS: u32 = 10;
        let mut ordered_ids = None;

        for attempt in 0..MAX_CYCLE_BREAKS {
            match graph.topological_sort() {
                Ok(ids) => {
                    ordered_ids = Some(ids);
                    break;
                }
                Err(HierarchicalError::CyclicDependency) => {
                    if attempt == 0 {
                        debug!("Detected cyclic dependencies in commit ordering");
                    }

                    if graph.break_cycles() {
                        debug!("Broke cycle (attempt {})", attempt + 1);
                    } else {
                        // No cycles to break, but sort failed - shouldn't happen
                        return Err(HierarchicalError::CyclicDependency);
                    }
                }
                Err(e) => return Err(e),
            }
        }

        let ordered_ids = ordered_ids.ok_or(HierarchicalError::CyclicDependency)?;

        // Build ordered result
        let commit_map: HashMap<PlannedCommitId, PlannedCommit> =
            commits.into_iter().map(|c| (c.id, c)).collect();

        let ordered: Vec<PlannedCommit> = ordered_ids
            .into_iter()
            .filter_map(|id| commit_map.get(&id).cloned())
            .collect();

        Ok(ordered)
    }

    /// Add dependencies based on change categories
    fn add_category_dependencies(
        commits: &[PlannedCommit],
        analysis: &AnalysisResults,
        graph: &mut DependencyGraph,
    ) {
        // Collect commits by category
        let mut by_category: HashMap<ChangeCategory, Vec<PlannedCommitId>> = HashMap::new();

        for commit in commits {
            let hunk_ids: Vec<HunkId> = commit
                .changes
                .iter()
                .filter_map(|c| {
                    if let PlannedChange::ExistingHunk(id) = c {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .collect();

            let categories: HashSet<ChangeCategory> = hunk_ids
                .iter()
                .filter_map(|id| analysis.get(*id))
                .map(|a| a.category)
                .collect();

            for cat in categories {
                by_category.entry(cat).or_default().push(commit.id);
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
        commits: &[PlannedCommit],
        analysis: &AnalysisResults,
        graph: &mut DependencyGraph,
    ) {
        // Group commits by the files they touch
        let mut commits_by_file: HashMap<String, Vec<(PlannedCommitId, usize)>> = HashMap::new();

        for commit in commits {
            let hunk_ids: Vec<HunkId> = commit
                .changes
                .iter()
                .filter_map(|c| {
                    if let PlannedChange::ExistingHunk(id) = c {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .collect();

            for (idx, hunk_id) in hunk_ids.iter().enumerate() {
                if let Some(a) = analysis.get(*hunk_id) {
                    commits_by_file
                        .entry(a.file_path.clone())
                        .or_default()
                        .push((commit.id, idx));
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
    nodes: HashSet<PlannedCommitId>,
    edges: HashMap<PlannedCommitId, HashSet<PlannedCommitId>>,
    reverse_edges: HashMap<PlannedCommitId, HashSet<PlannedCommitId>>,
}

impl DependencyGraph {
    fn new(commits: &[PlannedCommit]) -> Self {
        let nodes: HashSet<PlannedCommitId> = commits.iter().map(|c| c.id).collect();
        let edges: HashMap<PlannedCommitId, HashSet<PlannedCommitId>> =
            nodes.iter().map(|&id| (id, HashSet::new())).collect();
        let reverse_edges: HashMap<PlannedCommitId, HashSet<PlannedCommitId>> =
            nodes.iter().map(|&id| (id, HashSet::new())).collect();

        Self {
            nodes,
            edges,
            reverse_edges,
        }
    }

    fn add_edge(&mut self, from: PlannedCommitId, to: PlannedCommitId) {
        if !self.nodes.contains(&from) || !self.nodes.contains(&to) {
            return;
        }
        if from == to {
            return;
        }

        self.edges.entry(from).or_default().insert(to);
        self.reverse_edges.entry(to).or_default().insert(from);
    }

    fn topological_sort(&self) -> Result<Vec<PlannedCommitId>, HierarchicalError> {
        let mut in_degree: HashMap<PlannedCommitId, usize> = self
            .nodes
            .iter()
            .map(|&id| {
                let degree = self.reverse_edges.get(&id).map(|s| s.len()).unwrap_or(0);
                (id, degree)
            })
            .collect();

        // Start with nodes that have no incoming edges
        let mut queue: VecDeque<PlannedCommitId> = in_degree
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

    /// Find nodes involved in a cycle
    fn find_cycle_nodes(&self) -> HashSet<PlannedCommitId> {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();
        let mut cycle_nodes = HashSet::new();

        for &node in &self.nodes {
            if !visited.contains(&node) {
                self.dfs_find_cycle(node, &mut visited, &mut rec_stack, &mut cycle_nodes);
            }
        }

        cycle_nodes
    }

    fn dfs_find_cycle(
        &self,
        node: PlannedCommitId,
        visited: &mut HashSet<PlannedCommitId>,
        rec_stack: &mut HashSet<PlannedCommitId>,
        cycle_nodes: &mut HashSet<PlannedCommitId>,
    ) -> bool {
        visited.insert(node);
        rec_stack.insert(node);

        if let Some(neighbors) = self.edges.get(&node) {
            for &neighbor in neighbors {
                if !visited.contains(&neighbor) {
                    if self.dfs_find_cycle(neighbor, visited, rec_stack, cycle_nodes) {
                        cycle_nodes.insert(node);
                        return true;
                    }
                } else if rec_stack.contains(&neighbor) {
                    // Found a cycle
                    cycle_nodes.insert(node);
                    cycle_nodes.insert(neighbor);
                    return true;
                }
            }
        }

        rec_stack.remove(&node);
        false
    }

    /// Remove all edges involving nodes in the cycle
    fn break_cycles(&mut self) -> bool {
        let cycle_nodes = self.find_cycle_nodes();

        if cycle_nodes.is_empty() {
            return false;
        }

        let mut edges_removed = false;

        // Remove edges between cycle nodes
        for &from_node in &cycle_nodes {
            if let Some(neighbors) = self.edges.get_mut(&from_node) {
                let to_remove: Vec<PlannedCommitId> = neighbors
                    .iter()
                    .filter(|&&to| cycle_nodes.contains(&to))
                    .copied()
                    .collect();

                for to_node in to_remove {
                    neighbors.remove(&to_node);
                    if let Some(rev) = self.reverse_edges.get_mut(&to_node) {
                        rev.remove(&from_node);
                    }
                    edges_removed = true;
                }
            }
        }

        edges_removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CommitDescription, HunkId, PlannedChange};

    fn make_commit(id: usize, hunk_ids: Vec<usize>, depends_on: Vec<usize>) -> PlannedCommit {
        PlannedCommit::with_dependencies(
            PlannedCommitId(id),
            CommitDescription::new(
                format!("Commit {}", id),
                format!("Long message for commit {}", id),
            ),
            hunk_ids
                .into_iter()
                .map(|h| PlannedChange::ExistingHunk(HunkId(h)))
                .collect(),
            depends_on.into_iter().map(PlannedCommitId).collect(),
        )
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
        let ids: Vec<usize> = ordered.iter().map(|c| c.id.0).collect();
        assert_eq!(ids, vec![0, 1, 2]);
    }

    #[test]
    fn test_cyclic_dependency_resolution() {
        let commits = vec![
            make_commit(0, vec![0], vec![1]),
            make_commit(1, vec![1], vec![0]),
        ];

        let analysis = AnalysisResults::new();

        let result = GlobalOrderer::order(commits, &analysis);

        // Should succeed by breaking the cycle
        assert!(result.is_ok());
        let ordered = result.unwrap();
        assert_eq!(ordered.len(), 2);
    }

    #[test]
    fn test_empty_commits() {
        let commits: Vec<PlannedCommit> = vec![];
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
