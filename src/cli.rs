use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::features::Feature;

#[derive(Parser, Debug)]
#[command(name = "git-reabsorb")]
#[command(about = "Reorganize git commits by unstaging and recommitting")]
#[command(version)]
#[command(subcommand_required = false)]
pub struct Cli {
    #[command(flatten)]
    pub llm: LlmArgs,

    #[command(flatten)]
    pub plan: PlanArgs,

    #[command(flatten)]
    pub execution: ExecutionArgs,

    /// Enable experimental features (comma-separated)
    /// Available: attempt-validation-fix
    /// Can also be set via GIT_REABSORB_FEATURES env var
    #[arg(
        long = "features",
        global = true,
        value_delimiter = ',',
        env = "GIT_REABSORB_FEATURES"
    )]
    pub features: Option<Vec<Feature>>,

    /// Increase verbosity (-v for debug, -vv for trace with LLM streaming)
    #[arg(short = 'v', long = "verbose", global = true, action = clap::ArgAction::Count)]
    pub verbosity: u8,

    /// Suppress informational output (errors only)
    #[arg(short, long, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Global LLM configuration options.
#[derive(Args, Debug, Clone, Default)]
pub struct LlmArgs {
    /// LLM provider to use (claude, opencode)
    /// Can also be set via GIT_REABSORB_LLM_PROVIDER env var
    #[arg(
        long = "llm-provider",
        global = true,
        env = "GIT_REABSORB_LLM_PROVIDER"
    )]
    pub provider: Option<String>,

    /// LLM model to use (provider-specific)
    /// Can also be set via GIT_REABSORB_LLM_MODEL env var
    #[arg(long = "llm-model", global = true, env = "GIT_REABSORB_LLM_MODEL")]
    pub model: Option<String>,

    /// Backend for opencode provider (e.g., lmstudio, ollama)
    /// Can also be set via GIT_REABSORB_OPENCODE_BACKEND env var
    #[arg(
        long = "opencode-backend",
        global = true,
        env = "GIT_REABSORB_OPENCODE_BACKEND"
    )]
    pub opencode_backend: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Generate a plan and save it (use 'apply' to execute)
    Plan(PlanArgs),
    /// Apply a previously saved plan
    Apply(ApplyArgs),
    /// Reset to the pre-reabsorb ref created during planning
    Reset,
    /// Show status of current plan (for debugging)
    Status,
    /// Assess commit quality in a range
    Assess(AssessArgs),
    /// Compare two saved assessments
    Compare(CompareArgs),
}

/// Shared args for commit execution (used by both plan+apply and apply)
#[derive(Args, Debug, Clone, Default)]
pub struct ExecutionArgs {
    /// Skip pre-commit and commit-msg hooks
    #[arg(long)]
    pub no_verify: bool,

    /// Use planned messages without opening an editor
    #[arg(long = "no-editor")]
    pub no_editor: bool,
}

#[derive(Args, Debug, Clone)]
pub struct PlanArgs {
    /// Commit range to reabsorb (default: auto-detect branch base..HEAD)
    /// Examples: main..HEAD, HEAD~5..HEAD, abc123..def456, or just 'main' (implies main..HEAD)
    #[arg(value_name = "RANGE")]
    pub range: Option<String>,

    /// Base branch to reabsorb from (uses tip of branch)
    /// Examples: main, develop, origin/main, feat/my-feature
    #[arg(short, long)]
    pub base: Option<String>,

    /// Reorganization strategy
    #[arg(short = 's', long, value_enum, default_value = "preserve")]
    pub strategy: crate::models::Strategy,

    /// Show plan without executing
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Save plan to disk for later execution with 'apply'
    #[arg(long = "save-plan")]
    pub save_plan: bool,
}

#[derive(Args, Debug)]
pub struct ApplyArgs {
    /// Resume a partially-applied plan
    #[arg(long)]
    pub resume: bool,

    #[command(flatten)]
    pub execution: ExecutionArgs,
}

#[derive(Args, Debug, Clone)]
pub struct AssessArgs {
    /// Commit range to assess (default: auto-detect branch base..HEAD)
    /// Examples: main..HEAD, HEAD~5..HEAD, abc123..def456, or just 'main' (implies main..HEAD)
    #[arg(value_name = "RANGE")]
    pub range: Option<String>,

    /// Base branch to assess from
    #[arg(short, long)]
    pub base: Option<String>,

    /// Criteria to assess (default: all)
    /// Options: atomicity, message_quality, logical_cohesion, scope, reversibility
    #[arg(short, long, value_delimiter = ',')]
    pub criteria: Option<Vec<String>>,

    /// Output format
    #[arg(short, long, value_enum, default_value = "pretty")]
    pub format: OutputFormat,

    /// Save assessment to file (default: .git/reabsorb/assessments/<timestamp>.json)
    #[arg(long)]
    pub save: Option<Option<PathBuf>>,

    /// Compare against a previous assessment
    #[arg(long)]
    pub compare: Option<PathBuf>,

    /// Show full rationale and evidence in output
    #[arg(long)]
    pub full: bool,

    /// Maximum parallel commit assessments (default: 4)
    #[arg(short = 'j', long, default_value = "4")]
    pub parallel: usize,
}

#[derive(Args, Debug, Clone)]
pub struct CompareArgs {
    /// Path to the "before" assessment file
    #[arg(value_name = "BEFORE")]
    pub before: PathBuf,

    /// Path to the "after" assessment file
    #[arg(value_name = "AFTER")]
    pub after: PathBuf,

    /// Output format
    #[arg(short, long, value_enum, default_value = "pretty")]
    pub format: OutputFormat,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
pub enum OutputFormat {
    /// Human-readable formatted output
    #[default]
    Pretty,
    /// JSON output
    Json,
    /// Markdown report
    Markdown,
    /// Compact single-line per commit
    Compact,
}
