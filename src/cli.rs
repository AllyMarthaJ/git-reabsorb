use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "git-reabsorb")]
#[command(about = "Reorganize git commits by unstaging and recommitting")]
#[command(version)]
#[command(subcommand_required = false)]
pub struct Cli {
    #[command(flatten)]
    pub default_plan: PlanArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Generate a plan for the selected range (applies by default)
    Plan(PlanArgs),
    /// Apply a previously saved plan
    Apply(ApplyArgs),
    /// Reset to the pre-reabsorb ref created during planning
    Reset,
}

#[derive(Args, Debug, Clone)]
pub struct PlanArgs {
    /// Commit range to reabsorb (default: auto-detect branch base..HEAD)
    /// Examples: main..HEAD, HEAD~5..HEAD, abc123..def456
    #[arg(value_name = "RANGE")]
    pub range: Option<String>,

    /// Base branch to reabsorb from (uses tip of branch)
    /// Examples: main, develop, origin/main, feat/my-feature
    #[arg(short, long)]
    pub base: Option<String>,

    /// Reorganization strategy
    #[arg(short = 's', long, value_enum, default_value = "preserve")]
    pub strategy: StrategyArg,

    /// Show plan without executing
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Save plan to disk without applying it
    #[arg(long = "save-plan", alias = "plan-only")]
    pub save_plan: bool,

    /// Skip pre-commit and commit-msg hooks
    #[arg(long)]
    pub no_verify: bool,

    /// Use planned messages without opening an editor
    #[arg(long = "no-editor")]
    pub no_editor: bool,
}

#[derive(Args, Debug)]
pub struct ApplyArgs {
    /// Resume a partially-applied plan
    #[arg(long)]
    pub resume: bool,

    /// Skip pre-commit and commit-msg hooks
    #[arg(long)]
    pub no_verify: bool,

    /// Use the planned commit messages without opening an editor
    #[arg(long = "no-editor")]
    pub no_editor: bool,
}

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
