use crate::cli::{ApplyOpts, Command, ScrambleOpts};
use crate::diff_parser::DiffParseError;
use crate::editor::{Editor, EditorError};
use crate::git::{GitError, GitOps, PRE_SCRAMBLE_REF};
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
    #[error("{0}")]
    User(String),
}

pub struct App<G: GitOps, E: Editor, P: PlanStore> {
    git: G,
    editor: E,
    plan_store: P,
    strategies: StrategyFactory,
}

impl<G: GitOps, E: Editor, P: PlanStore> App<G, E, P> {
    pub fn new(git: G, editor: E, plan_store: P, strategies: StrategyFactory) -> Self {
        Self {
            git,
            editor,
            plan_store,
            strategies,
        }
    }

    pub fn run(&mut self, command: Command) -> Result<(), AppError> {
        match command {
            Command::Reset => self.handle_reset(),
            Command::Apply(opts) => self.handle_apply(opts),
            Command::Scramble(opts) => self.handle_scramble(opts),
        }
    }

    fn handle_reset(&mut self) -> Result<(), AppError> {
        if !self.git.has_pre_scramble_head() {
            return Err(AppError::User(
                "No pre-scramble state found. Nothing to reset.".to_string(),
            ));
        }

        let pre_scramble_head = self.git.get_pre_scramble_head()?;
        println!(
            "Resetting from {} to pre-scramble state {}",
            short_sha(&self.git.get_head()?),
            short_sha(&pre_scramble_head)
        );

        self.git.reset_hard(&pre_scramble_head)?;
        self.git.clear_pre_scramble_head()?;

        println!("Successfully reset to pre-scramble state.");
        println!("The saved ref ({}) has been cleared.", PRE_SCRAMBLE_REF);

        Ok(())
    }

    fn handle_apply(&mut self, opts: ApplyOpts) -> Result<(), AppError> {
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
            return Err(AppError::User(format!(
                "Plan has {} commits already applied. Use --resume to continue, or delete .git/scramble/plan.json",
                plan.next_commit_index
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
            &mut plan,
        ) {
            eprintln!("\nError during commit creation: {}", err);
            eprintln!("Progress saved. Use --resume to continue.");
            return Err(AppError::Execution(err));
        }

        self.plan_store.delete()?;
        println!(
            "\nDone! Created {} commits.",
            plan.next_commit_index.saturating_sub(already_created)
        );
        println!("To undo: git-scramble --reset");

        Ok(())
    }

    fn handle_scramble(&mut self, opts: ScrambleOpts) -> Result<(), AppError> {
        if self.plan_store.exists() {
            eprintln!("Warning: A saved plan exists. Use --apply/--resume or delete .git/scramble/plan.json\n");
        }

        let range =
            RangeResolver::new(&self.git).resolve(opts.range.as_deref(), opts.base.as_deref())?;
        println!(
            "Scrambling {}..{}",
            short_sha(&range.base),
            short_sha(&range.head)
        );

        if self.git.has_pre_scramble_head() {
            eprintln!(
                "Warning: Pre-scramble state exists ({}). Use --reset or it will be overwritten.\n",
                short_sha(&self.git.get_pre_scramble_head()?)
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

        self.git.save_pre_scramble_head()?;
        println!("Saved pre-scramble state to {}", PRE_SCRAMBLE_REF);

        println!("Resetting to {}...", short_sha(&range.base));
        self.git.reset_to(&range.base)?;

        let diff_output = self.git.get_working_tree_diff()?;
        let hunks = planner.parse_diff_with_commit_mapping(&diff_output, &file_to_commits)?;
        println!("Parsed {} hunks", hunks.len());

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

        if opts.plan_only {
            println!(
                "Plan saved to {}",
                crate::reorganize::plan_file::plan_file_path().display()
            );
            println!("\nTo apply: git-scramble --apply");
            println!("To undo reset: git-scramble --reset");
            return Ok(());
        }

        let executor = PlanExecutor::new(&self.git, &self.editor, &self.plan_store);
        if let Err(err) = executor.execute(
            &plan.hunks,
            &plan.planned_commits,
            &plan.new_files_to_commits,
            opts.no_verify,
            &mut saved_plan,
        ) {
            eprintln!("\nError: {}", err);
            eprintln!("Progress saved. Use --resume to continue, or --reset to undo.");
            return Err(AppError::Execution(err));
        }

        self.plan_store.delete()?;
        println!("\nDone! Created {} commits.", plan.planned_commits.len());
        println!("To undo: git-scramble --reset");

        Ok(())
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
