use clap::Parser;
use log::LevelFilter;

use git_reabsorb::app::{App, StrategyFactory};
use git_reabsorb::cli::{Cli, Command};
use git_reabsorb::editor::SystemEditor;
use git_reabsorb::features::Features;
use git_reabsorb::git::{Git, GitOps};
use git_reabsorb::llm::{LlmConfig, LlmProvider};
use git_reabsorb::plan_store::FilePlanStore;

fn main() {
    let cli = Cli::parse();

    // Initialize logging based on verbosity flags
    let log_level = if cli.quiet {
        LevelFilter::Error
    } else {
        match cli.verbosity {
            0 => LevelFilter::Info,
            1 => LevelFilter::Debug,
            _ => LevelFilter::Trace,
        }
    };
    env_logger::Builder::new()
        .filter_level(log_level)
        .format_target(false)
        .format_timestamp(None)
        .init();

    // Initialize feature flags from environment, then apply CLI overrides
    let features = Features::from_env().with_overrides(cli.features.as_deref());
    Features::init_global(features);

    // Build LLM config from environment, then apply CLI overrides
    let provider = cli
        .llm
        .provider
        .as_ref()
        .and_then(|s| s.parse::<LlmProvider>().ok());
    let llm_config = LlmConfig::from_env().with_overrides(
        provider,
        cli.llm.model.clone(),
        cli.llm.opencode_backend.clone(),
    );

    let git = Git::with_repo_root().expect("Not a git repository");
    let editor = SystemEditor::new();
    let namespace = determine_namespace(&git);
    let plan_store = FilePlanStore::new(namespace.clone());
    let strategies = StrategyFactory::new().with_llm_config(llm_config.clone());

    let mut app = App::new(
        git,
        editor,
        plan_store,
        strategies,
        llm_config,
        namespace.clone(),
    );
    match cli.command {
        Some(cmd) => {
            if let Err(err) = app.run(cmd) {
                log::error!("{}", err);
                std::process::exit(1);
            }
        }
        None => {
            // No subcommand: plan (with save) then apply
            let mut plan_args = cli.default_plan.clone();
            plan_args.save_plan = true;

            if let Err(err) = app.run(Command::Plan(plan_args.clone())) {
                log::error!("{}", err);
                std::process::exit(1);
            }

            // If dry-run was specified, don't apply
            if cli.default_plan.dry_run {
                return;
            }

            let apply_args = git_reabsorb::cli::ApplyArgs {
                resume: false,
                no_verify: plan_args.no_verify,
                no_editor: plan_args.no_editor,
            };

            if let Err(err) = app.run(Command::Apply(apply_args)) {
                log::error!("{}", err);
                std::process::exit(1);
            }
        }
    }
}

fn determine_namespace(git: &Git) -> String {
    let branch = git
        .current_branch_name()
        .unwrap_or_else(|_| "HEAD".to_string());
    if branch == "HEAD" {
        "detached".to_string()
    } else {
        sanitize(&branch)
    }
}

fn sanitize(input: &str) -> String {
    let lowered = input.to_ascii_lowercase();
    let cleaned: String = lowered
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "branch".to_string()
    } else {
        trimmed
    }
}
