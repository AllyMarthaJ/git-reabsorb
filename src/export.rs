//! Training data export from labeled assessment data.
//!
//! Converts labeled NDJSON (output of `assess --from-file`) into standard
//! fine-tuning formats: SFT, DPO, and assessment JSONL.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Arc;

use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::extract::ExtractedCommit;
use crate::llm::LlmClient;

/// Assessment label attached to an extracted commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabeledCommit {
    #[serde(flatten)]
    pub commit: ExtractedCommit,
    pub assessment: AssessmentLabel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssessmentLabel {
    pub level: u8,
    pub rationale: String,
    pub evidence: Vec<String>,
    pub suggestions: Vec<String>,
}

/// SFT training record: diff -> good message.
#[derive(Debug, Serialize)]
pub struct SftRecord {
    pub diff: String,
    pub files_changed: Vec<String>,
    pub diff_stat: crate::assessment::criteria::DiffStats,
    pub message: String,
}

/// DPO training record: diff, chosen message, rejected message.
#[derive(Debug, Serialize)]
pub struct DpoRecord {
    pub diff: String,
    pub chosen: String,
    pub rejected: String,
}

/// Assessment training record: diff + message -> score + rationale.
#[derive(Debug, Serialize)]
pub struct AssessmentRecord {
    pub diff: String,
    pub message: String,
    pub level: u8,
    pub rationale: String,
    pub suggestions: Vec<String>,
}

/// Export configuration.
pub struct ExportConfig {
    pub min_level: u8,
    pub output_dir: std::path::PathBuf,
}

/// Read labeled NDJSON and produce SFT training data.
pub fn export_sft(input: &Path, config: &ExportConfig) -> Result<usize, ExportError> {
    let output_path = config.output_dir.join("sft.jsonl");
    fs::create_dir_all(&config.output_dir).map_err(|e| ExportError::Io(e.to_string()))?;

    let reader = BufReader::new(
        fs::File::open(input).map_err(|e| ExportError::Io(e.to_string()))?,
    );
    let mut writer = fs::File::create(&output_path).map_err(|e| ExportError::Io(e.to_string()))?;

    let mut count = 0;
    for line in reader.lines() {
        let line = line.map_err(|e| ExportError::Io(e.to_string()))?;
        let labeled: LabeledCommit =
            serde_json::from_str(&line).map_err(|e| ExportError::Parse(e.to_string()))?;

        if labeled.assessment.level >= config.min_level {
            let message = if labeled.commit.body.is_empty() {
                labeled.commit.subject.clone()
            } else {
                format!("{}\n\n{}", labeled.commit.subject, labeled.commit.body)
            };

            let record = SftRecord {
                diff: labeled.commit.diff,
                files_changed: labeled.commit.files_changed,
                diff_stat: labeled.commit.diff_stat,
                message,
            };

            let json = serde_json::to_string(&record)
                .map_err(|e| ExportError::Serialization(e.to_string()))?;
            writeln!(writer, "{}", json).map_err(|e| ExportError::Io(e.to_string()))?;
            count += 1;
        }
    }

    info!("Wrote {} SFT records to {}", count, output_path.display());
    Ok(count)
}

/// Export DPO training data by generating synthetic "bad" messages via LLM.
///
/// For each SFT-eligible commit (level >= min_level), prompts the LLM to write
/// a realistic level-2 message for the same diff — terse, restates the what,
/// omits the why. The good message becomes `chosen`, the synthetic one `rejected`.
pub fn export_dpo(
    input: &Path,
    config: &ExportConfig,
    client: Arc<dyn LlmClient>,
) -> Result<usize, ExportError> {
    let output_path = config.output_dir.join("dpo.jsonl");
    fs::create_dir_all(&config.output_dir).map_err(|e| ExportError::Io(e.to_string()))?;

    let reader = BufReader::new(
        fs::File::open(input).map_err(|e| ExportError::Io(e.to_string()))?,
    );
    let mut writer = fs::File::create(&output_path).map_err(|e| ExportError::Io(e.to_string()))?;

    let mut count = 0;
    for line in reader.lines() {
        let line = line.map_err(|e| ExportError::Io(e.to_string()))?;
        let labeled: LabeledCommit =
            serde_json::from_str(&line).map_err(|e| ExportError::Parse(e.to_string()))?;

        if labeled.assessment.level < config.min_level {
            continue;
        }

        let chosen = if labeled.commit.body.is_empty() {
            labeled.commit.subject.clone()
        } else {
            format!("{}\n\n{}", labeled.commit.subject, labeled.commit.body)
        };

        // Truncate diff for the prompt to keep costs down
        let diff_for_prompt = if labeled.commit.diff.len() > 3000 {
            let safe = labeled.commit.diff.floor_char_boundary(3000);
            match labeled.commit.diff[..safe].rfind('\n') {
                Some(pos) => &labeled.commit.diff[..pos],
                None => &labeled.commit.diff[..safe],
            }
        } else {
            &labeled.commit.diff
        };

        let prompt = format!(
            r#"You are generating training data for a commit message quality model.

Given this diff, write a realistic but LOW-QUALITY commit message (level 2 out of 5).

Level 2 characteristics:
- Title names the action but not the system or reason (e.g. "Update files", "Fix bug", "Increase threads")
- No body, or body just restates the diff in prose
- Fails the grep test: a teammate could not find this with a reasonable search term
- No context a future reader couldn't get from the diff itself

Diff:
```
{}
```

Files changed: {}

Write ONLY the commit message, nothing else. No markdown, no explanation. Just the message as it would appear in `git log`."#,
            diff_for_prompt,
            labeled.commit.files_changed.join(", ")
        );

        match client.complete(&prompt) {
            Ok(rejected) => {
                let rejected = rejected.trim().to_string();
                let record = DpoRecord {
                    diff: labeled.commit.diff,
                    chosen,
                    rejected,
                };
                let json = serde_json::to_string(&record)
                    .map_err(|e| ExportError::Serialization(e.to_string()))?;
                writeln!(writer, "{}", json).map_err(|e| ExportError::Io(e.to_string()))?;
                count += 1;
            }
            Err(e) => {
                warn!(
                    "Failed to generate rejection for {}: {}",
                    &labeled.commit.hash[..8.min(labeled.commit.hash.len())],
                    e
                );
            }
        }

        if count % 50 == 0 && count > 0 {
            info!("Generated {} DPO records...", count);
        }
    }

    info!("Wrote {} DPO records to {}", count, output_path.display());
    Ok(count)
}

/// Export assessment training data (all levels).
pub fn export_assessment(input: &Path, config: &ExportConfig) -> Result<usize, ExportError> {
    let output_path = config.output_dir.join("assessment.jsonl");
    fs::create_dir_all(&config.output_dir).map_err(|e| ExportError::Io(e.to_string()))?;

    let reader = BufReader::new(
        fs::File::open(input).map_err(|e| ExportError::Io(e.to_string()))?,
    );
    let mut writer = fs::File::create(&output_path).map_err(|e| ExportError::Io(e.to_string()))?;

    let mut count = 0;
    for line in reader.lines() {
        let line = line.map_err(|e| ExportError::Io(e.to_string()))?;
        let labeled: LabeledCommit =
            serde_json::from_str(&line).map_err(|e| ExportError::Parse(e.to_string()))?;

        let message = if labeled.commit.body.is_empty() {
            labeled.commit.subject.clone()
        } else {
            format!("{}\n\n{}", labeled.commit.subject, labeled.commit.body)
        };

        let record = AssessmentRecord {
            diff: labeled.commit.diff,
            message,
            level: labeled.assessment.level,
            rationale: labeled.assessment.rationale,
            suggestions: labeled.assessment.suggestions,
        };

        let json = serde_json::to_string(&record)
            .map_err(|e| ExportError::Serialization(e.to_string()))?;
        writeln!(writer, "{}", json).map_err(|e| ExportError::Io(e.to_string()))?;
        count += 1;
    }

    info!(
        "Wrote {} assessment records to {}",
        count,
        output_path.display()
    );
    Ok(count)
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// Write metadata about the export.
pub fn write_metadata(
    config: &ExportConfig,
    sft_count: usize,
    dpo_count: usize,
    assessment_count: usize,
) -> Result<(), ExportError> {
    let metadata = serde_json::json!({
        "created_at": chrono::Utc::now().to_rfc3339(),
        "min_level": config.min_level,
        "sft_records": sft_count,
        "dpo_records": dpo_count,
        "assessment_records": assessment_count,
    });

    let path = config.output_dir.join("metadata.json");
    let json =
        serde_json::to_string_pretty(&metadata).map_err(|e| ExportError::Serialization(e.to_string()))?;
    fs::write(&path, json).map_err(|e| ExportError::Io(e.to_string()))?;

    info!("Wrote metadata to {}", path.display());
    Ok(())
}
