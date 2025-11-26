use clap::{Parser, ValueEnum};

/// Command line interface definition for git-scramble.
#[derive(Parser, Debug)]
#[command(name = "git-scramble")]
#[command(about = "Reorganize git commits by unstaging and recommitting")]
#[command(version)]
pub struct Cli {
    /// Commit range to scramble (default: auto-detect branch base..HEAD)
    /// Examples: main..HEAD, HEAD~5..HEAD, abc123..def456
    #[arg(value_name = "RANGE", conflicts_with_all = ["reset", "base"])]
    range: Option<String>,

    /// Base branch to scramble from (uses tip of branch)
    /// Examples: main, develop, origin/main, feat/my-feature
    #[arg(short, long, conflicts_with_all = ["reset", "range"])]
    base: Option<String>,

    /// Reorganization strategy
    #[arg(
        short,
        long,
        value_enum,
        default_value = "preserve",
        conflicts_with = "reset"
    )]
    strategy: StrategyArg,

    /// Show plan without executing
    #[arg(long, conflicts_with = "reset")]
    dry_run: bool,

    /// Reset to the pre-scramble state (undo the last scramble)
    #[arg(long)]
    reset: bool,

    /// Skip pre-commit and commit-msg hooks
    #[arg(long, conflicts_with = "reset")]
    no_verify: bool,

    /// Generate plan and save it without applying (works with any strategy)
    #[arg(long, conflicts_with_all = ["reset", "resume", "apply", "dry_run"])]
    plan_only: bool,

    /// Apply a saved plan from the beginning
    #[arg(long, conflicts_with_all = ["reset", "plan_only", "resume", "strategy", "range", "base", "dry_run"])]
    apply: bool,

    /// Resume applying a partially completed plan
    #[arg(long, conflicts_with_all = ["reset", "plan_only", "apply", "strategy", "range", "base", "dry_run"])]
    resume: bool,
}

impl Cli {
    /// Convert parsed CLI flags into a concrete command for the application layer.
    pub fn into_command(self) -> Command {
        if self.reset {
            return Command::Reset;
        }

        if self.apply {
            return Command::Apply(ApplyOpts {
                resume: false,
                no_verify: self.no_verify,
            });
        }

        if self.resume {
            return Command::Apply(ApplyOpts {
                resume: true,
                no_verify: self.no_verify,
            });
        }

        Command::Scramble(ScrambleOpts {
            range: self.range,
            base: self.base,
            strategy: self.strategy,
            dry_run: self.dry_run,
            no_verify: self.no_verify,
            plan_only: self.plan_only,
        })
    }
}

/// Available strategies for generating a new commit plan.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum StrategyArg {
    /// Preserve original commit structure
    Preserve,
    /// Group changes by file (one commit per file)
    ByFile,
    /// Squash all changes into a single commit
    Squash,
    /// Use LLM to intelligently reorganize commits
    Llm,
}

/// Command selected by CLI parsing.
pub enum Command {
    Reset,
    Apply(ApplyOpts),
    Scramble(ScrambleOpts),
}

/// Options for applying a saved plan.
pub struct ApplyOpts {
    pub resume: bool,
    pub no_verify: bool,
}

/// Options for running the scramble workflow directly.
pub struct ScrambleOpts {
    pub range: Option<String>,
    pub base: Option<String>,
    pub strategy: StrategyArg,
    pub dry_run: bool,
    pub no_verify: bool,
    pub plan_only: bool,
}
