//! Assessment comparison and persistence.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::assessment::types::{AssessmentComparison, RangeAssessment};

const DEFAULT_ASSESSMENTS_DIR: &str = ".git/reabsorb/assessments";

/// Get the default assessment storage directory.
pub fn default_assessments_dir() -> PathBuf {
    PathBuf::from(DEFAULT_ASSESSMENTS_DIR)
}

/// Generate a default filename for an assessment.
pub fn default_assessment_filename() -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    format!("assessment_{}.json", timestamp)
}

/// Save an assessment to disk.
///
/// If `path` is None, saves to the default location.
pub fn save_assessment(
    assessment: &RangeAssessment,
    path: Option<&Path>,
) -> Result<PathBuf, std::io::Error> {
    let save_path = match path {
        Some(p) => p.to_path_buf(),
        None => {
            let dir = default_assessments_dir();
            fs::create_dir_all(&dir)?;
            dir.join(default_assessment_filename())
        }
    };

    // Create parent directories if needed
    if let Some(parent) = save_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(assessment).map_err(std::io::Error::other)?;

    fs::write(&save_path, json)?;
    Ok(save_path)
}

/// Load a saved assessment.
pub fn load_assessment(path: &Path) -> Result<RangeAssessment, std::io::Error> {
    let json = fs::read_to_string(path)?;
    serde_json::from_str(&json).map_err(std::io::Error::other)
}

/// List saved assessments in the default directory.
pub fn list_assessments() -> Result<Vec<PathBuf>, std::io::Error> {
    let dir = default_assessments_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut assessments = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            assessments.push(path);
        }
    }

    // Sort by filename (which includes timestamp)
    assessments.sort();
    Ok(assessments)
}

/// Compare two assessments.
pub fn compare_assessments(
    before: RangeAssessment,
    after: RangeAssessment,
) -> AssessmentComparison {
    let overall_delta = after.overall_score - before.overall_score;

    let mut criterion_deltas = HashMap::new();
    for (criterion_id, after_agg) in &after.aggregate_scores {
        if let Some(before_agg) = before.aggregate_scores.get(criterion_id) {
            criterion_deltas.insert(
                criterion_id.clone(),
                after_agg.mean_score - before_agg.mean_score,
            );
        }
    }

    let improvements = criterion_deltas
        .iter()
        .filter(|(_, delta)| **delta > 0.1)
        .map(|(id, delta)| format!("{}: +{:.2}", id, delta))
        .collect();

    let regressions = criterion_deltas
        .iter()
        .filter(|(_, delta)| **delta < -0.1)
        .map(|(id, delta)| format!("{}: {:.2}", id, delta))
        .collect();

    AssessmentComparison {
        before,
        after,
        overall_delta,
        criterion_deltas,
        improvements,
        regressions,
    }
}

/// Delete a saved assessment.
pub fn delete_assessment(path: &Path) -> Result<(), std::io::Error> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assessment::types::RangeAssessment;
    use std::collections::HashMap;

    fn make_assessment(overall: f32) -> RangeAssessment {
        RangeAssessment {
            base_sha: "base".to_string(),
            head_sha: "head".to_string(),
            assessed_at: "2024-01-01T00:00:00Z".to_string(),
            commit_assessments: vec![],
            aggregate_scores: HashMap::new(),
            overall_score: overall,
            range_observations: vec![],
        }
    }

    #[test]
    fn compare_shows_improvement() {
        let before = make_assessment(0.5);
        let after = make_assessment(0.8);

        let comparison = compare_assessments(before, after);

        assert!((comparison.overall_delta - 0.3).abs() < 0.001);
    }

    #[test]
    fn compare_shows_regression() {
        let before = make_assessment(0.8);
        let after = make_assessment(0.5);

        let comparison = compare_assessments(before, after);

        assert!((comparison.overall_delta - (-0.3)).abs() < 0.001);
    }
}
