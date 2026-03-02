mod executor;
mod planner;

use log::{error, info, warn};

use crate::assessment::{self, AssessmentEngine, CriterionId};
use crate::cancel;
use crate::cli::{
    ApplyArgs, AssessArgs, AssessCommand, Command, CommitRange, CompareArgs,
    ExportTrainingDataArgs, ExtractArgs, OutputFormat, PlanArgs, RewordArgs, TrainingDataFormat,
};
use crate::editor::{Editor, EditorError};
use crate::features::Feature;
use crate::git::{GitError, GitOps};
use crate::llm::{LlmConfig, ToolCapability};
use crate::models::{PlannedCommit, Strategy};
use crate::patch::ParseError;
use crate::plan_store::{PlanFileError, PlanStore, SavedPlan};
use crate::reorganize::{
    Absorb, ApplyResult, GroupByFile, HierarchicalReorganizer, LlmReorganizer, PreserveOriginal,
    ReorganizeError, Reorganizer, Squash,
};
use crate::utils::short_sha;

pub use executor::{ExecutionError, PlanExecutor};
pub use planner::{PlanDraft, Planner};

/// Factory for instantiating reorganizers from CLI strategy argument.
#[derive(Clone, Default)]
pub struct StrategyFactory {
    llm_config: LlmConfig,
}

impl StrategyFactory {
    pub fn new() -> Self {
        Self {
            llm_config: LlmConfig::default(),
        }
    }

    pub fn with_llm_config(mut self, config: LlmConfig) -> Self {
        self.llm_config = config;
        self
    }

    pub fn create(&self, strategy: Strategy) -> Box<dyn Reorganizer> {
        match strategy {
            Strategy::Preserve => Box::new(PreserveOriginal),
            Strategy::ByFile => Box::new(GroupByFile),
            Strategy::Squash => Box::new(Squash),
            Strategy::Llm => {
                let config = self.config_with_file_io_tools();
                Box::new(LlmReorganizer::new(config.create_boxed_client()))
            }
            Strategy::Hierarchical => {
                let config = self.config_with_file_io_tools();
                let client = config.create_client();
                Box::new(HierarchicalReorganizer::new(Some(client)))
            }
            Strategy::Absorb => Box::new(Absorb),
        }
    }

    /// Returns config with FileIo capability if FileBasedLlmIo feature is enabled.
    fn config_with_file_io_tools(&self) -> LlmConfig {
        if Feature::FileBasedLlmIo.is_enabled() {
            self.llm_config
                .clone()
                .with_capabilities(vec![ToolCapability::FileIo])
        } else {
            self.llm_config.clone()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Editor(#[from] EditorError),
    #[error(transparent)]
    Plan(#[from] PlanFileError),
    #[error(transparent)]
    Reorg(#[from] ReorganizeError),
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Execution(#[from] ExecutionError),
    #[error(transparent)]
    Assessment(#[from] assessment::AssessmentError),
    #[error("Integrity check failed: {0}")]
    Integrity(String),
    #[error("{0}")]
    User(String),
}

pub struct App<G: GitOps, E: Editor, P: PlanStore> {
    git: G,
    editor: E,
    plan_store: P,
    strategies: StrategyFactory,
    llm_config: LlmConfig,
    namespace: String,
    pre_reabsorb_ref: String,
}

impl<G: GitOps, E: Editor, P: PlanStore> App<G, E, P> {
    pub fn new(
        git: G,
        editor: E,
        plan_store: P,
        strategies: StrategyFactory,
        llm_config: LlmConfig,
        namespace: String,
    ) -> Self {
        let pre_reabsorb_ref = crate::git::pre_reabsorb_ref_for(&namespace);
        Self {
            git,
            editor,
            plan_store,
            strategies,
            llm_config,
            namespace,
            pre_reabsorb_ref,
        }
    }

    pub fn run(&mut self, command: Command) -> Result<(), AppError> {
        match command {
            Command::Reset => self.handle_reset(),
            Command::Apply(opts) => self.handle_apply(opts),
            Command::Plan(opts) => self.handle_plan(opts),
            Command::Status => self.handle_status(),
            Command::Assess(opts) => self.handle_assess(opts),
            Command::Compare(opts) => self.handle_compare(opts),
            Command::Reword(opts) => self.handle_reword(opts),
        }
    }

    fn handle_reset(&mut self) -> Result<(), AppError> {
        if !self.git.has_pre_reabsorb_head(&self.pre_reabsorb_ref) {
            return Err(AppError::User(
                "No pre-reabsorb state found. Nothing to reset.".to_string(),
            ));
        }

        let pre_reabsorb_head = self.git.get_pre_reabsorb_head(&self.pre_reabsorb_ref)?;
        info!(
            "Resetting from {} to pre-reabsorb state {}",
            short_sha(&self.git.get_head()?),
            short_sha(&pre_reabsorb_head)
        );

        self.git.reset_hard(&pre_reabsorb_head)?;
        self.git.clear_pre_reabsorb_head(&self.pre_reabsorb_ref)?;

        info!("Successfully reset to pre-reabsorb state.");
        info!(
            "The saved ref ({}) has been cleared.",
            self.pre_reabsorb_ref
        );

        Ok(())
    }

    fn handle_apply(&mut self, opts: ApplyArgs) -> Result<(), AppError> {
        let mut plan = self.plan_store.load()?;

        // Let the strategy handle apply if it wants to (e.g., absorb calls git-absorb directly)
        let reorganizer = self.strategies.create(plan.strategy);
        let result = reorganizer.apply(&self.git, &[])?;
        if result == ApplyResult::Handled {
            self.plan_store.delete()?;
            info!("Strategy '{:?}' handled apply directly.", plan.strategy);
            return Ok(());
        }

        let already_created = plan.next_commit_index;

        if opts.resume {
            if plan.is_complete() {
                info!("Plan is already complete. Nothing to resume.");
                self.plan_store.delete()?;
                return Ok(());
            }
            info!(
                "Resuming plan: {}/{} commits already created",
                plan.next_commit_index,
                plan.commits.len()
            );
        } else if plan.next_commit_index > 0 {
            let plan_path = crate::plan_store::plan_file_path(&self.namespace);
            return Err(AppError::User(format!(
                "Plan has {} commits already applied. Use 'git reabsorb apply --resume' to continue, or delete {}",
                plan.next_commit_index,
                plan_path.display()
            )));
        } else {
            info!("Applying saved plan (strategy: {:?})", plan.strategy);
        }

        // For fresh apply (not resume), we need to reset to base
        if !opts.resume {
            // Check for existing pre-reabsorb state
            if self.git.has_pre_reabsorb_head(&self.pre_reabsorb_ref) {
                warn!(
                    "Pre-reabsorb state exists ({}). Use 'git reabsorb reset' or it will be overwritten.",
                    short_sha(&self.git.get_pre_reabsorb_head(&self.pre_reabsorb_ref)?)
                );
            }

            // Verify we're at the expected HEAD (the original_head from when plan was saved)
            let current_head = self.git.get_head()?;
            if current_head != plan.original_head {
                warn!(
                    "HEAD ({}) differs from plan's original HEAD ({})",
                    short_sha(&current_head),
                    short_sha(&plan.original_head)
                );
            }

            // Save pre-reabsorb state and reset to base
            self.git.save_pre_reabsorb_head(&self.pre_reabsorb_ref)?;
            info!("Saved pre-reabsorb state to {}", self.pre_reabsorb_ref);

            info!("Resetting to {}...", short_sha(&plan.base_sha));
            self.git.reset_to(&plan.base_sha)?;
        }

        let hunks = plan.get_working_tree_hunks();
        let file_changes = plan.get_file_changes();
        let planned_commits = plan.to_planned_commits();
        print_planned_commits(
            &planned_commits[plan.next_commit_index..],
            plan.next_commit_index,
        );

        cancel::register_handler();

        let executor = PlanExecutor::new(&self.git, &self.editor, &self.plan_store);
        if let Err(err) = executor.execute(
            &hunks,
            &planned_commits,
            &file_changes,
            opts.execution.no_verify,
            opts.execution.no_editor,
            &mut plan,
        ) {
            // Handle cancellation by resetting to pre-reabsorb state
            if matches!(err, ExecutionError::Cancelled) {
                warn!("Cancelled. Resetting to pre-reabsorb state...");
                if let Err(reset_err) = self.reset_to_pre_reabsorb() {
                    error!("Failed to reset: {}", reset_err);
                }
                return Err(AppError::User("Cancelled by user".to_string()));
            }

            error!("Commit creation failed: {}", err);
            info!("Progress saved. Use 'git reabsorb apply --resume' to continue.");
            return Err(AppError::Execution(err));
        }

        self.verify_final_state(&plan.original_head)?;
        self.plan_store.delete()?;
        info!(
            "Done! Created {} commits.",
            plan.next_commit_index.saturating_sub(already_created)
        );
        info!("To undo: git reabsorb reset");

        Ok(())
    }

    fn handle_plan(&mut self, opts: PlanArgs) -> Result<(), AppError> {
        if self.plan_store.exists() {
            let plan_path = crate::plan_store::plan_file_path(&self.namespace);
            warn!(
                "A saved plan exists. Use 'git reabsorb apply' or delete {}",
                plan_path.display()
            );
        }

        let range = CommitRange::resolve(opts.range.as_ref(), opts.base.as_deref(), &self.git)?;
        info!(
            "Planning {}..{}",
            short_sha(&range.base),
            short_sha(range.head())
        );

        let planner = Planner::new(&self.git, self.strategies.clone());
        let source_commits = planner.read_source_commits(&range.base, range.head())?;
        info!("Found {} commits", source_commits.len());

        let file_to_commits = planner.build_file_to_commits_map(&source_commits)?;

        // Get the diff between base and head (doesn't modify working tree)
        let diff_output = self.git.diff_trees(&range.base, range.head())?;
        let (hunks, file_changes) =
            planner.parse_diff_full_with_commit_mapping(&diff_output, &file_to_commits)?;
        info!("Parsed {} hunks", hunks.len());
        let binary_count = file_changes.iter().filter(|fc| fc.is_binary).count();
        if binary_count > 0 {
            info!("Found {} binary files", binary_count);
        }
        let mode_count = file_changes
            .iter()
            .filter(|fc| !fc.is_binary && !fc.has_content_hunks)
            .count();
        if mode_count > 0 {
            info!("Found {} mode changes", mode_count);
        }

        let plan = planner.draft_plan(
            opts.strategy,
            &source_commits,
            &hunks,
            &file_to_commits,
            &file_changes,
        )?;
        info!("Strategy: {:?}", plan.strategy);
        print_planned_commits(&plan.planned_commits, 0);

        // Dry run: just show the plan, no disk writes
        if opts.dry_run {
            return Ok(());
        }

        // Save plan to disk
        if opts.save_plan {
            let saved_plan = SavedPlan::new(
                plan.strategy,
                range.base.clone(),
                range.head().to_string(),
                &plan.planned_commits,
                &plan.hunks,
                &plan.file_to_commits,
                &plan.file_changes,
            );
            self.plan_store.save(&saved_plan)?;
            info!(
                "Plan saved to {}",
                crate::plan_store::plan_file_path(&self.namespace).display()
            );
            info!("To apply: git reabsorb apply");
        }

        Ok(())
    }

    fn verify_final_state(&self, expected_head: &str) -> Result<(), AppError> {
        let current_head = self.git.get_head()?;
        let diff = self.git.diff_trees(expected_head, &current_head)?;
        if diff.trim().is_empty() {
            Ok(())
        } else {
            Err(AppError::Integrity(format!(
                "HEAD {} differs from expected {}",
                short_sha(&current_head),
                short_sha(expected_head)
            )))
        }
    }

    /// Reset to pre-reabsorb state and clean up.
    fn reset_to_pre_reabsorb(&self) -> Result<(), AppError> {
        if !self.git.has_pre_reabsorb_head(&self.pre_reabsorb_ref) {
            return Ok(()); // Nothing to reset to
        }

        let pre_reabsorb_head = self.git.get_pre_reabsorb_head(&self.pre_reabsorb_ref)?;
        self.git.reset_hard(&pre_reabsorb_head)?;
        self.git.clear_pre_reabsorb_head(&self.pre_reabsorb_ref)?;
        self.plan_store.delete().ok(); // Ignore errors cleaning up plan

        info!(
            "Reset to pre-reabsorb state ({})",
            short_sha(&pre_reabsorb_head)
        );
        Ok(())
    }

    fn handle_status(&mut self) -> Result<(), AppError> {
        info!("=== Git Reabsorb Status ===");

        // Current git state
        let head = self.git.get_head()?;
        info!("Current HEAD: {}", short_sha(&head));

        if let Ok(branch) = self.git.current_branch_name() {
            info!("Current branch: {}", branch);
        }

        // Pre-reabsorb state
        info!("--- Pre-reabsorb State ---");
        if self.git.has_pre_reabsorb_head(&self.pre_reabsorb_ref) {
            let pre = self.git.get_pre_reabsorb_head(&self.pre_reabsorb_ref)?;
            info!(
                "Pre-reabsorb ref: {} -> {}",
                self.pre_reabsorb_ref,
                short_sha(&pre)
            );
        } else {
            info!("No pre-reabsorb state saved");
        }

        // Plan state
        info!("--- Saved Plan ---");
        if !self.plan_store.exists() {
            info!("No saved plan found");
            return Ok(());
        }

        let plan = self.plan_store.load()?;
        info!("Strategy: {:?}", plan.strategy);
        info!("Base SHA: {}", short_sha(&plan.base_sha));
        info!("Original HEAD: {}", short_sha(&plan.original_head));
        info!(
            "Progress: {}/{} commits",
            plan.next_commit_index,
            plan.commits.len()
        );

        // Show commits
        info!("--- Planned Commits ---");
        for (i, commit) in plan.commits.iter().enumerate() {
            let status = if i < plan.next_commit_index {
                if let Some(sha) = &commit.created_sha {
                    format!("[DONE: {}]", short_sha(sha))
                } else {
                    "[DONE]".to_string()
                }
            } else if i == plan.next_commit_index {
                "[NEXT]".to_string()
            } else {
                "[PENDING]".to_string()
            };
            info!(
                "  {}. {} \"{}\" ({} changes)",
                i + 1,
                status,
                commit.description.short,
                commit.changes.len()
            );
        }

        // If there's a next commit, show details
        if plan.next_commit_index < plan.commits.len() {
            let next_commit = &plan.commits[plan.next_commit_index];
            info!("--- Next Commit Details ---");
            info!("Message: {}", next_commit.description.short);
            info!("Changes: {} hunks", next_commit.changes.len());

            // Show files involved
            let hunks = plan.get_working_tree_hunks();
            let planned_commits = plan.to_planned_commits();
            let planned = &planned_commits[plan.next_commit_index];

            let mut files: std::collections::BTreeSet<&std::path::Path> =
                std::collections::BTreeSet::new();
            for change in &planned.changes {
                if let Some(hunk) = change.resolve(&hunks) {
                    files.insert(&hunk.file_path);
                }
            }
            info!("Files:");
            for file in files {
                // Check if file is in index
                let in_index = self.git.file_in_index(file).unwrap_or(false);
                info!("  {} (in_index={})", file.display(), in_index);
            }
        }

        // Show all files in index for debugging
        info!("--- Git Index Status ---");
        if let Ok(output) = self.git.run_git_output(&["ls-files"]) {
            let files: Vec<&str> = output.lines().take(20).collect();
            info!("Files in index (first 20):");
            for f in &files {
                info!("  {}", f);
            }
            let total = output.lines().count();
            if total > 20 {
                info!("  ... and {} more", total - 20);
            }
        }

        Ok(())
    }

    fn handle_assess(&mut self, opts: AssessArgs) -> Result<(), AppError> {
        use std::collections::HashSet;
        use std::io::{BufRead, BufReader, Write};

        use crate::export::{AssessmentLabel, LabeledCommit};
        use crate::extract::ExtractedCommit;

        // Dispatch subcommands
        match &opts.subcommand {
            Some(AssessCommand::Extract(extract_opts)) => {
                return self.handle_extract(extract_opts.clone());
            }
            Some(AssessCommand::Export(export_opts)) => {
                return self.handle_export_training_data(export_opts.clone());
            }
            None => {}
        }

        let from_file = opts.from_file.is_some();

        // Parse criteria — default depends on mode
        let criterion_ids = match &opts.criteria {
            Some(names) => {
                let mut ids = Vec::new();
                for name in names {
                    let id: CriterionId = name.parse().map_err(AppError::User)?;
                    ids.push(id);
                }
                ids
            }
            None if from_file => vec![CriterionId::MessageQuality],
            None => CriterionId::all().to_vec(),
        };

        // Build ExtractedCommit records from either source
        let (records, base_sha, head_sha) = if let Some(input_path) = &opts.from_file {
            // Load already-labeled hashes for resumability
            let mut done_hashes = HashSet::new();
            if let Some(output_path) = &opts.output {
                if output_path.exists() {
                    let reader = BufReader::new(
                        std::fs::File::open(output_path)
                            .map_err(|e| AppError::User(format!("Failed to open output: {}", e)))?,
                    );
                    for line in reader.lines() {
                        let line =
                            line.map_err(|e| AppError::User(format!("Read error: {}", e)))?;
                        if let Ok(labeled) = serde_json::from_str::<LabeledCommit>(&line) {
                            done_hashes.insert(labeled.commit.hash.clone());
                        }
                    }
                    if !done_hashes.is_empty() {
                        info!("Resuming: {} commits already labeled", done_hashes.len());
                    }
                }
            }

            let reader = BufReader::new(
                std::fs::File::open(input_path)
                    .map_err(|e| AppError::User(format!("Failed to open input: {}", e)))?,
            );
            let mut records = Vec::new();
            for (i, line) in reader.lines().enumerate() {
                let line = line.map_err(|e| AppError::User(format!("Read error: {}", e)))?;
                let record: ExtractedCommit = serde_json::from_str(&line)
                    .map_err(|e| {
                        AppError::User(format!("Parse error on line {}: {}", i + 1, e))
                    })?;
                if !done_hashes.contains(&record.hash) {
                    records.push(record);
                }
            }
            if records.is_empty() {
                info!("All commits already assessed");
                return Ok(());
            }
            info!(
                "Assessing {} commits from {}",
                records.len(),
                input_path.display()
            );
            let first = records.first().map(|r| r.hash.clone()).unwrap_or_default();
            let last = records.last().map(|r| r.hash.clone()).unwrap_or_default();
            (records, first, last)
        } else {
            let range =
                CommitRange::resolve(opts.range.as_ref(), opts.base.as_deref(), &self.git)?;
            info!(
                "Assessing commits {}..{}",
                short_sha(&range.base),
                short_sha(range.head())
            );

            let commits = self.git.read_commits(&range.base, range.head())?;
            if commits.is_empty() {
                return Err(AppError::User("No commits found in range".to_string()));
            }
            info!("Found {} commits to assess", commits.len());

            let base_sha = range.base.clone();
            let head_sha = range.head().to_string();

            let mut records = Vec::new();
            for commit in &commits {
                let hunks = self
                    .git
                    .read_hunks(&commit.sha, 0)
                    .map_err(|e| AppError::User(e.to_string()))?;
                let diff = hunks
                    .iter()
                    .map(|h| h.to_patch())
                    .collect::<Vec<_>>()
                    .join("\n");
                let files_changed = self
                    .git
                    .get_files_changed_in_commit(&commit.sha)
                    .unwrap_or_default();

                let mut lines_added = 0usize;
                let mut lines_removed = 0usize;
                for hunk in &hunks {
                    for line in &hunk.lines {
                        match line {
                            crate::models::DiffLine::Added(_) => lines_added += 1,
                            crate::models::DiffLine::Removed(_) => lines_removed += 1,
                            crate::models::DiffLine::Context(_) => {}
                        }
                    }
                }

                let body = if commit.message.long.len() > commit.message.short.len() {
                    commit.message.long[commit.message.short.len()..]
                        .trim()
                        .to_string()
                } else {
                    String::new()
                };

                records.push(ExtractedCommit {
                    hash: commit.sha.clone(),
                    subject: commit.message.short.clone(),
                    body,
                    author_name: String::new(),
                    author_date: String::new(),
                    diff,
                    diff_stat: crate::assessment::DiffStats {
                        files_changed: files_changed.len(),
                        lines_added,
                        lines_removed,
                    },
                    files_changed,
                    diff_truncated: false,
                });
            }
            (records, base_sha, head_sha)
        };

        // Assess in batches, writing results incrementally
        let client = self.llm_config.create_client();
        let engine =
            AssessmentEngine::new(client, &criterion_ids).with_parallelism(opts.parallel);

        let batch_size = opts.parallel.max(1) * 2;
        let total = records.len();
        let mut all_assessments = Vec::with_capacity(total);

        let mut writer: Option<std::fs::File> = if let Some(output_path) = &opts.output {
            Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(output_path)
                    .map_err(|e| AppError::User(format!("Failed to open output: {}", e)))?,
            )
        } else {
            None
        };

        for chunk_start in (0..total).step_by(batch_size) {
            let chunk_end = (chunk_start + batch_size).min(total);
            let batch = &records[chunk_start..chunk_end];

            info!("[{}/{}] Assessing batch...", chunk_start + 1, total);

            let batch_assessments = engine
                .assess_commits(batch)
                .map_err(|e| AppError::User(e.to_string()))?;

            // Write this batch immediately
            if let Some(ref mut w) = writer {
                for (record, assessment) in batch.iter().zip(&batch_assessments) {
                    let primary = assessment.criterion_scores.first();
                    let labeled = LabeledCommit {
                        commit: record.clone(),
                        assessment: AssessmentLabel {
                            level: primary.map(|s| s.level).unwrap_or(0),
                            rationale: primary.map(|s| s.rationale.clone()).unwrap_or_default(),
                            evidence: primary.map(|s| s.evidence.clone()).unwrap_or_default(),
                            suggestions: primary.map(|s| s.suggestions.clone()).unwrap_or_default(),
                        },
                    };
                    let json = serde_json::to_string(&labeled)
                        .map_err(|e| AppError::User(format!("Serialization error: {}", e)))?;
                    writeln!(w, "{}", json)
                        .map_err(|e| AppError::User(format!("Write error: {}", e)))?;
                }
                w.flush()
                    .map_err(|e| AppError::User(format!("Flush error: {}", e)))?;
            }

            all_assessments.extend(batch_assessments);
        }

        let commit_assessments = all_assessments;
        if let Some(output_path) = &opts.output {
            info!("Labeled {} commits to {}", total, output_path.display());
        }

        // Build RangeAssessment for display/save
        let aggregate_scores = engine.calculate_aggregates(&commit_assessments);
        let overall_score = if commit_assessments.is_empty() {
            0.0
        } else {
            commit_assessments
                .iter()
                .map(|ca| ca.overall_score)
                .sum::<f32>()
                / commit_assessments.len() as f32
        };
        let result = assessment::types::RangeAssessment {
            base_sha,
            head_sha,
            assessed_at: chrono::Utc::now().to_rfc3339(),
            commit_assessments,
            aggregate_scores,
            overall_score,
            range_observations: Vec::new(),
        };

        // Display
        if let Some(compare_path) = &opts.compare {
            let previous = assessment::load_assessment(compare_path)
                .map_err(|e| AppError::User(format!("Failed to load comparison: {}", e)))?;
            let comparison = assessment::compare_assessments(previous, result.clone());
            let output =
                assessment::report::format_comparison(&comparison, convert_format(opts.format));
            println!("{}", output);
        } else {
            let output = assessment::report::format_assessment(
                &result,
                convert_format(opts.format),
                opts.full,
            );
            println!("{}", output);
        }

        if let Some(save_path) = opts.save {
            let path = assessment::save_assessment(&result, save_path.as_deref())
                .map_err(|e| AppError::User(format!("Failed to save assessment: {}", e)))?;
            info!("Assessment saved to: {}", path.display());
        }

        Ok(())
    }

    fn handle_compare(&self, opts: CompareArgs) -> Result<(), AppError> {
        let before = assessment::load_assessment(&opts.before)
            .map_err(|e| AppError::User(format!("Failed to load 'before' assessment: {}", e)))?;

        let after = assessment::load_assessment(&opts.after)
            .map_err(|e| AppError::User(format!("Failed to load 'after' assessment: {}", e)))?;

        let comparison = assessment::compare_assessments(before, after);
        let output =
            assessment::report::format_comparison(&comparison, convert_format(opts.format));
        println!("{}", output);

        Ok(())
    }

    fn handle_reword(&mut self, opts: RewordArgs) -> Result<(), AppError> {
        use crate::reorganize::llm::{build_reword_prompt, FixMessageResponse};
        use crate::utils::extract_json_str;

        // Use MessageQuality if no criteria specified
        let criteria = if opts.criteria.is_empty() {
            vec![CriterionId::MessageQuality]
        } else {
            opts.criteria.clone()
        };

        info!(
            "Rewording commits in range: {:?} (criteria: {:?})",
            opts.range, criteria
        );

        if opts.dry_run {
            info!("Dry run mode - no changes will be made");
        }

        // Resolve the range (handles single refs by getting parent as base)
        let range = opts.range.resolve_single_or_range(&self.git)?;

        // Read commits in range
        let commits = self.git.read_commits(&range.base, range.head())?;
        if commits.is_empty() {
            return Err(AppError::User("No commits found in range".to_string()));
        }

        info!("Found {} commits to analyze", commits.len());

        // Create assessment engine
        let client = self.llm_config.create_client();
        let engine = AssessmentEngine::new(client.clone(), &criteria);

        // Assess commits
        let assessment_result =
            engine.assess_range(&self.git, &range.base, range.head(), &commits)?;

        // Track proposed rewrites
        let mut rewrites: Vec<(String, String, String, String)> = Vec::new(); // (sha, old_short, new_short, new_long)

        // For each commit assessment, generate improved message
        for (commit, ca) in commits
            .iter()
            .zip(assessment_result.commit_assessments.iter())
        {
            info!(
                "  {} {}: {:.1}%",
                short_sha(&commit.sha),
                commit.message.short,
                ca.overall_score * 100.0
            );

            // Get diff for context
            let diff_content = self
                .git
                .read_hunks(&commit.sha, 0)
                .map(|hunks| {
                    hunks
                        .iter()
                        .map(|h| h.to_patch())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();

            // Build prompt and get improved message
            let prompt = build_reword_prompt(commit, ca, &diff_content);

            match client.complete(&prompt) {
                Ok(response) => {
                    if let Some(json_str) = extract_json_str(&response) {
                        if let Ok(fix) = serde_json::from_str::<FixMessageResponse>(json_str) {
                            rewrites.push((
                                commit.sha.clone(),
                                commit.message.short.clone(),
                                fix.description.short.clone(),
                                fix.description.long.clone(),
                            ));
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to reword commit {}: {}", short_sha(&commit.sha), e);
                }
            }
        }

        // Show proposed rewrites
        if rewrites.is_empty() {
            info!("No rewrites proposed");
            return Ok(());
        }

        info!("\n=== Proposed Rewrites ===\n");
        for (sha, old_short, new_short, new_long) in &rewrites {
            info!("Commit {}", short_sha(sha));
            info!("  Before: {}", old_short);
            info!("  After:  {}", new_short);
            if !new_long.is_empty() {
                let indented: String = new_long
                    .lines()
                    .map(|line| format!("    {}", line))
                    .collect::<Vec<_>>()
                    .join("\n");
                info!("  Body:\n{}", indented);
            }
            info!("");
        }

        if opts.dry_run {
            info!("Dry run complete. To apply, run without --dry-run");
            info!(
                "Note: Rewriting commits requires an interactive rebase.\n\
                   You can manually apply these changes with:\n\
                   git rebase -i {}",
                short_sha(&range.base)
            );
        } else {
            // For now, just show the instructions - actual git rebase is complex
            info!(
                "To apply these changes, run:\n\
                   git rebase -i {}\n\n\
                   Then change 'pick' to 'reword' for each commit you want to update.",
                short_sha(&range.base)
            );
        }

        Ok(())
    }

    fn handle_extract(&mut self, opts: ExtractArgs) -> Result<(), AppError> {
        use crate::extract::{self, ExtractConfig};

        let range = CommitRange::resolve(opts.range.as_ref(), opts.base.as_deref(), &self.git)?;

        info!(
            "Extracting commits {}..{}",
            short_sha(&range.base),
            short_sha(range.head())
        );

        let config = ExtractConfig {
            max_diff_size: opts.max_diff_size,
            max_files: opts.max_files,
        };

        let mut writer: Box<dyn std::io::Write> = if let Some(path) = &opts.output {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| AppError::User(format!("Failed to create directory: {}", e)))?;
            }
            Box::new(
                std::fs::File::create(path)
                    .map_err(|e| AppError::User(format!("Failed to create output file: {}", e)))?,
            )
        } else {
            Box::new(std::io::stdout())
        };

        let count = extract::extract_range(&self.git, &range.base, range.head(), &config, &mut writer)
            .map_err(|e| AppError::User(e.to_string()))?;

        info!("Extracted {} commits", count);
        if let Some(path) = &opts.output {
            info!("Written to {}", path.display());
        }

        Ok(())
    }

    fn handle_export_training_data(&self, opts: ExportTrainingDataArgs) -> Result<(), AppError> {
        use crate::export::{self, ExportConfig};

        let config = ExportConfig {
            min_level: opts.min_level,
            output_dir: opts.output_dir.clone(),
        };

        let client = self.llm_config.create_client();
        let mut sft_count = 0;
        let mut dpo_count = 0;
        let mut assessment_count = 0;

        match opts.format {
            TrainingDataFormat::Sft => {
                sft_count = export::export_sft(&opts.input, &config)
                    .map_err(|e| AppError::User(e.to_string()))?;
            }
            TrainingDataFormat::Assessment => {
                assessment_count = export::export_assessment(&opts.input, &config)
                    .map_err(|e| AppError::User(e.to_string()))?;
            }
            TrainingDataFormat::Dpo => {
                dpo_count = export::export_dpo(&opts.input, &config, client)
                    .map_err(|e| AppError::User(e.to_string()))?;
            }
            TrainingDataFormat::All => {
                sft_count = export::export_sft(&opts.input, &config)
                    .map_err(|e| AppError::User(e.to_string()))?;
                dpo_count = export::export_dpo(&opts.input, &config, client)
                    .map_err(|e| AppError::User(e.to_string()))?;
                assessment_count = export::export_assessment(&opts.input, &config)
                    .map_err(|e| AppError::User(e.to_string()))?;
            }
        }

        export::write_metadata(&config, sft_count, dpo_count, assessment_count)
            .map_err(|e| AppError::User(e.to_string()))?;

        Ok(())
    }
}

fn print_planned_commits(commits: &[PlannedCommit], offset: usize) {
    info!("Planned {} commits:", commits.len());
    for (i, commit) in commits.iter().enumerate() {
        info!(
            "  {}. \"{}\" ({} changes)",
            offset + i + 1,
            commit.description.short,
            commit.changes.len()
        );
    }
}

fn convert_format(format: OutputFormat) -> assessment::report::OutputFormat {
    match format {
        OutputFormat::Pretty => assessment::report::OutputFormat::Pretty,
        OutputFormat::Json => assessment::report::OutputFormat::Json,
        OutputFormat::Markdown => assessment::report::OutputFormat::Markdown,
        OutputFormat::Compact => assessment::report::OutputFormat::Compact,
    }
}
