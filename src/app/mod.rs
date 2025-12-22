mod executor;
mod planner;

use log::{error, info, warn};

use crate::assessment::{self, AssessmentEngine, CriterionId};
use crate::cancel;
use crate::cli::{ApplyArgs, AssessArgs, Command, CompareArgs, OutputFormat, PlanArgs};
use crate::patch::ParseError;
use crate::editor::{Editor, EditorError};
use crate::features::Feature;
use crate::git::{GitError, GitOps};
use crate::llm::{LlmConfig, ToolCapability};
use crate::models::{PlannedCommit, Strategy};
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

/// Inclusive/exclusive commit range (base is exclusive, head inclusive).
#[derive(Clone, Debug)]
pub struct CommitRange {
    pub base: String,
    pub head: String,
}

fn resolve_range<G: GitOps>(
    git: &G,
    range: Option<&str>,
    base_branch: Option<&str>,
) -> Result<CommitRange, GitError> {
    match (range, base_branch) {
        (Some(r), None) => {
            if let Some((base, head)) = r.split_once("..") {
                // Explicit range: base..head
                Ok(CommitRange {
                    base: git.resolve_ref(base)?,
                    head: git.resolve_ref(head)?,
                })
            } else {
                // Single ref: treat as base..HEAD
                Ok(CommitRange {
                    base: git.resolve_ref(r)?,
                    head: git.get_head()?,
                })
            }
        }
        (None, Some(branch)) => Ok(CommitRange {
            base: git.resolve_ref(branch)?,
            head: git.get_head()?,
        }),
        (None, None) => Ok(CommitRange {
            base: git.find_branch_base()?,
            head: git.get_head()?,
        }),
        (Some(_), Some(_)) => Err(GitError::CommandFailed(
            "Cannot specify both range and --base".to_string(),
        )),
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

        let range = resolve_range(&self.git, opts.range.as_deref(), opts.base.as_deref())?;
        info!(
            "Planning {}..{}",
            short_sha(&range.base),
            short_sha(&range.head)
        );

        let planner = Planner::new(&self.git, self.strategies.clone());
        let source_commits = planner.read_source_commits(&range.base, &range.head)?;
        info!("Found {} commits", source_commits.len());

        let file_to_commits = planner.build_file_to_commits_map(&source_commits)?;

        // Get the diff between base and head (doesn't modify working tree)
        let diff_output = self.git.diff_trees(&range.base, &range.head)?;
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
                range.head.clone(),
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
        // Resolve commit range
        let range = resolve_range(&self.git, opts.range.as_deref(), opts.base.as_deref())?;

        info!(
            "Assessing commits {}..{}",
            short_sha(&range.base),
            short_sha(&range.head)
        );

        // Read commits
        let commits = self.git.read_commits(&range.base, &range.head)?;
        if commits.is_empty() {
            return Err(AppError::User("No commits found in range".to_string()));
        }

        info!("Found {} commits to assess", commits.len());

        // Parse criteria from args or use all
        let criterion_ids = match &opts.criteria {
            Some(names) => {
                let mut ids = Vec::new();
                for name in names {
                    let id: CriterionId = name.parse().map_err(AppError::User)?;
                    ids.push(id);
                }
                ids
            }
            None => CriterionId::all().to_vec(),
        };

        // Create assessment engine with parallelism
        let client = self.llm_config.create_client();
        let engine = AssessmentEngine::new(client, &criterion_ids).with_parallelism(opts.parallel);

        // Run assessment
        let result = engine.assess_range(&self.git, &range.base, &range.head, &commits)?;

        // Handle comparison if requested
        if let Some(compare_path) = &opts.compare {
            let previous = assessment::load_assessment(compare_path)
                .map_err(|e| AppError::User(format!("Failed to load comparison: {}", e)))?;

            let comparison = assessment::compare_assessments(previous, result.clone());
            let output =
                assessment::report::format_comparison(&comparison, convert_format(opts.format));
            println!("{}", output);
        } else {
            // Format and print assessment
            let output = assessment::report::format_assessment(
                &result,
                convert_format(opts.format),
                opts.full,
            );
            println!("{}", output);
        }

        // Save if requested
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
