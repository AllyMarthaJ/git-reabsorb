use std::path::PathBuf;
use std::str::FromStr;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::assessment::criteria::CriterionId;
use crate::features::Feature;
use crate::git::{GitError, GitOps};

/// Commit range (base is exclusive, head is inclusive).
///
/// Can be parsed from:
/// - An explicit range like "main..HEAD" or "abc123..def456"
/// - A single ref like "main" (implies main..HEAD, with head resolved later)
#[derive(Clone, Debug)]
pub struct CommitRange {
    pub base: String,
    /// None means "HEAD" (resolved later)
    pub head: Option<String>,
}

impl FromStr for CommitRange {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((base, head)) = s.split_once("..") {
            Ok(CommitRange {
                base: base.to_string(),
                head: Some(head.to_string()),
            })
        } else {
            Ok(CommitRange {
                base: s.to_string(),
                head: None,
            })
        }
    }
}

impl CommitRange {
    /// Resolve refs to actual commit SHAs.
    ///
    /// The `base_override` parameter allows the `--base` flag to override
    /// the base ref. Returns error if both range and --base are specified.
    pub fn resolve<G: GitOps>(
        range: Option<&CommitRange>,
        base_override: Option<&str>,
        git: &G,
    ) -> Result<CommitRange, GitError> {
        match (range, base_override) {
            (Some(r), None) => {
                let base = git.resolve_ref(&r.base)?;
                let head = match &r.head {
                    Some(h) => git.resolve_ref(h)?,
                    None => git.get_head()?,
                };
                Ok(CommitRange {
                    base,
                    head: Some(head),
                })
            }
            (None, Some(branch)) => Ok(CommitRange {
                base: git.resolve_ref(branch)?,
                head: Some(git.get_head()?),
            }),
            (None, None) => Ok(CommitRange {
                base: git.find_branch_base()?,
                head: Some(git.get_head()?),
            }),
            (Some(_), Some(_)) => Err(GitError::CommandFailed(
                "Cannot specify both range and --base".to_string(),
            )),
        }
    }

    /// Get the head SHA, panics if not resolved yet.
    pub fn head(&self) -> &str {
        self.head.as_ref().expect("CommitRange not resolved")
    }

    /// Resolve for a single commit (used by reword command).
    ///
    /// When head is None (single ref), returns that commit and its parent.
    /// When head is Some (range), resolves both refs.
    pub fn resolve_single_or_range<G: GitOps>(&self, git: &G) -> Result<CommitRange, GitError> {
        match &self.head {
            Some(head) => Ok(CommitRange {
                base: git.resolve_ref(&self.base)?,
                head: Some(git.resolve_ref(head)?),
            }),
            None => {
                // Single ref - get parent as base
                let commit_sha = git.resolve_ref(&self.base)?;
                let parent_ref = format!("{}^", commit_sha);
                let parent_sha = git
                    .resolve_ref(&parent_ref)
                    .unwrap_or_else(|_| commit_sha.clone());
                Ok(CommitRange {
                    base: parent_sha,
                    head: Some(commit_sha),
                })
            }
        }
    }
}

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
    /// Reword commit messages using LLM
    Reword(RewordArgs),
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
    pub range: Option<CommitRange>,

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
    pub range: Option<CommitRange>,

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

#[derive(Args, Debug, Clone)]
pub struct RewordArgs {
    /// Commit range to reword (default: HEAD)
    /// Examples: HEAD~3..HEAD, main..HEAD, or a single commit SHA
    #[arg(default_value = "HEAD")]
    pub range: CommitRange,

    /// Criteria to improve (comma-separated, default: message_quality)
    #[arg(long, value_delimiter = ',')]
    pub criteria: Vec<CriterionId>,

    /// Show changes without applying
    #[arg(short = 'n', long)]
    pub dry_run: bool,
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
