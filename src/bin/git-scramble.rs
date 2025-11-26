use clap::Parser;

use git_scramble::app::App;
use git_scramble::cli::{Cli, Command};
use git_scramble::editor::SystemEditor;
use git_scramble::git::{Git, GitOps};
use git_scramble::plan_store::FilePlanStore;
use git_scramble::services::strategy::StrategyFactory;

fn main() {
    let cli = Cli::parse();

    let git = Git::new();
    let editor = SystemEditor::new();
    let namespace = determine_namespace(&git);
    let plan_store = FilePlanStore::new(namespace.clone());
    let strategies = StrategyFactory::new();

    let mut app = App::new(git, editor, plan_store, strategies, namespace.clone());
    let command = cli
        .command
        .unwrap_or(Command::Plan(cli.default_plan.clone()));

    if let Err(err) = app.run(command) {
        eprintln!("Error: {}", err);
        std::process::exit(1);
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
