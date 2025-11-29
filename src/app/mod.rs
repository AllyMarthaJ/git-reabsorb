use crate::cli::{ApplyArgs, Command, PlanArgs};
use crate::diff_parser::DiffParseError;
use crate::editor::{Editor, EditorError};
use crate::git::{GitError, GitOps};
use crate::models::PlannedCommit;
use crate::plan_store::{PlanFileError, PlanStore, SavedPlan};
use crate::reorganize::ReorganizeError;
use crate::services::executor::{ExecutionError, PlanExecutor};
use crate::services::planner::Planner;
use crate::services::range::RangeResolver;
use crate::services::strategy::StrategyFactory;

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
    Diff(#[from] DiffParseError),
    #[error(transparent)]
    Execution(#[from] ExecutionError),
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
    namespace: String,
    pre_reabsorb_ref: String,
}

impl<G: GitOps, E: Editor, P: PlanStore> App<G, E, P> {
    pub fn new(
        git: G,
        editor: E,
        plan_store: P,
        strategies: StrategyFactory,
        namespace: String,
    ) -> Self {
        let pre_reabsorb_ref = crate::git::pre_reabsorb_ref_for(&namespace);
        Self {
            git,
            editor,
            plan_store,
            strategies,
            namespace,
            pre_reabsorb_ref,
        }
    }

    pub fn run(&mut self, command: Command) -> Result<(), AppError> {
        match command {
            Command::Reset => self.handle_reset(),
            Command::Apply(opts) => self.handle_apply(opts),
            Command::Plan(opts) => self.handle_plan(opts),
        }
    }

    fn handle_reset(&mut self) -> Result<(), AppError> {
        if !self.git.has_pre_reabsorb_head(&self.pre_reabsorb_ref) {
            return Err(AppError::User(
                "No pre-reabsorb state found. Nothing to reset.".to_string(),
            ));
        }

        let pre_reabsorb_head = self.git.get_pre_reabsorb_head(&self.pre_reabsorb_ref)?;
        println!(
            "Resetting from {} to pre-reabsorb state {}",
            short_sha(&self.git.get_head()?),
            short_sha(&pre_reabsorb_head)
        );

        self.git.reset_hard(&pre_reabsorb_head)?;
        self.git.clear_pre_reabsorb_head(&self.pre_reabsorb_ref)?;

        println!("Successfully reset to pre-reabsorb state.");
        println!(
            "The saved ref ({}) has been cleared.",
            self.pre_reabsorb_ref
        );

        Ok(())
    }

    fn handle_apply(&mut self, opts: ApplyArgs) -> Result<(), AppError> {
        let mut plan = self.plan_store.load()?;
        let already_created = plan.next_commit_index;

        if opts.resume {
            if plan.is_complete() {
                println!("Plan is already complete. Nothing to resume.");
                self.plan_store.delete()?;
                return Ok(());
            }
            println!(
                "Resuming plan: {}/{} commits already created",
                plan.next_commit_index,
                plan.commits.len()
            );
        } else if plan.next_commit_index > 0 {
            let plan_path = crate::reorganize::plan_file::plan_file_path(&self.namespace);
            return Err(AppError::User(format!(
                "Plan has {} commits already applied. Use 'git reabsorb apply --resume' to continue, or delete {}",
                plan.next_commit_index,
                plan_path.display()
            )));
        } else {
            println!("Applying saved plan (strategy: {})", plan.strategy);
        }

        if !opts.resume && self.git.get_head()? != plan.base_sha {
            eprintln!(
                "Warning: HEAD ({}) differs from plan's base ({})",
                short_sha(&self.git.get_head()?),
                short_sha(&plan.base_sha)
            );
        }

        let hunks = plan.get_working_tree_hunks();
        let new_files_to_commits = plan.get_new_files_to_commits();
        let planned_commits = plan.to_planned_commits();
        print_planned_commits(
            &planned_commits[plan.next_commit_index..],
            plan.next_commit_index,
        );

        let executor = PlanExecutor::new(&self.git, &self.editor, &self.plan_store);
        if let Err(err) = executor.execute(
            &hunks,
            &planned_commits,
            &new_files_to_commits,
            opts.no_verify,
            opts.no_editor,
            &mut plan,
        ) {
            eprintln!("\nError during commit creation: {}", err);
            eprintln!("Progress saved. Use 'git reabsorb apply --resume' to continue.");
            return Err(AppError::Execution(err));
        }

        self.verify_final_state(&plan.original_head)?;
        self.plan_store.delete()?;
        println!(
            "\nDone! Created {} commits.",
            plan.next_commit_index.saturating_sub(already_created)
        );
        println!("To undo: git reabsorb reset");

        Ok(())
    }

    fn handle_plan(&mut self, opts: PlanArgs) -> Result<(), AppError> {
        if self.plan_store.exists() {
            let plan_path = crate::reorganize::plan_file::plan_file_path(&self.namespace);
            eprintln!(
                "Warning: A saved plan exists. Use 'git reabsorb apply' or delete {}\n",
                plan_path.display()
            );
        }

        let range =
            RangeResolver::new(&self.git).resolve(opts.range.as_deref(), opts.base.as_deref())?;
        println!(
            "Scrambling {}..{}",
            short_sha(&range.base),
            short_sha(&range.head)
        );

        if self.git.has_pre_reabsorb_head(&self.pre_reabsorb_ref) {
            eprintln!(
                "Warning: Pre-reabsorb state exists ({}). Use 'git reabsorb reset' or it will be overwritten.\n",
                short_sha(&self.git.get_pre_reabsorb_head(&self.pre_reabsorb_ref)?)
            );
        }

        let planner = Planner::new(&self.git, self.strategies);
        let source_commits = planner.read_source_commits(&range.base, &range.head)?;
        println!("Found {} commits", source_commits.len());

        let (file_to_commits, new_files_to_commits) =
            planner.build_file_to_commits_map(&source_commits)?;

        if opts.dry_run {
            let hunks = planner.read_hunks_from_source_commits(&source_commits)?;
            let plan = planner.draft_plan(
                opts.strategy,
                &source_commits,
                &hunks,
                &file_to_commits,
                &new_files_to_commits,
            )?;
            println!("Parsed {} hunks", hunks.len());
            println!("Strategy: {}", plan.strategy_name);
            print_planned_commits(&plan.planned_commits, 0);
            println!("--dry-run: no changes made.");
            return Ok(());
        }

        self.git.save_pre_reabsorb_head(&self.pre_reabsorb_ref)?;
        println!("Saved pre-reabsorb state to {}", self.pre_reabsorb_ref);

        // Get the diff between base and head BEFORE resetting
        // This ensures we capture new files that would become untracked after reset
        let diff_output = self.git.diff_trees(&range.base, &range.head)?;
        let hunks = planner.parse_diff_with_commit_mapping(&diff_output, &file_to_commits)?;
        println!("Parsed {} hunks", hunks.len());

        println!("Resetting to {}...", short_sha(&range.base));
        self.git.reset_to(&range.base)?;

        let plan = planner.draft_plan(
            opts.strategy,
            &source_commits,
            &hunks,
            &file_to_commits,
            &new_files_to_commits,
        )?;
        println!("Strategy: {}", plan.strategy_name);
        print_planned_commits(&plan.planned_commits, 0);

        let mut saved_plan = SavedPlan::new(
            plan.strategy_name.clone(),
            range.base.clone(),
            range.head.clone(),
            &plan.planned_commits,
            &plan.hunks,
            &plan.new_hunks,
            &plan.file_to_commits,
            &plan.new_files_to_commits,
        );
        self.plan_store.save(&saved_plan)?;

        if opts.save_plan {
            println!(
                "Plan saved to {}",
                crate::reorganize::plan_file::plan_file_path(&self.namespace).display()
            );
            println!("\nTo apply: git reabsorb apply");
            println!("To undo reset: git reabsorb reset");
            return Ok(());
        }

        let executor = PlanExecutor::new(&self.git, &self.editor, &self.plan_store);
        if let Err(err) = executor.execute(
            &plan.hunks,
            &plan.planned_commits,
            &plan.new_files_to_commits,
            opts.no_verify,
            opts.no_editor,
            &mut saved_plan,
        ) {
            eprintln!("\nError: {}", err);
            eprintln!(
                "Progress saved. Use 'git reabsorb apply --resume' to continue, or 'git reabsorb reset' to undo."
            );
            return Err(AppError::Execution(err));
        }

        self.verify_final_state(&saved_plan.original_head)?;
        self.plan_store.delete()?;
        println!("\nDone! Created {} commits.", plan.planned_commits.len());
        println!("To undo: git reabsorb reset");

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
}

fn print_planned_commits(commits: &[PlannedCommit], offset: usize) {
    println!("\nPlanned {} commits:", commits.len());
    for (i, commit) in commits.iter().enumerate() {
        println!(
            "  {}. \"{}\" ({} changes)",
            offset + i + 1,
            commit.description.short,
            commit.changes.len()
        );
    }
    println!();
}

fn short_sha(sha: &str) -> &str {
    &sha[..8.min(sha.len())]
}
