use clap::Parser;

use git_scramble::app::App;
use git_scramble::cli::Cli;
use git_scramble::editor::SystemEditor;
use git_scramble::git::Git;
use git_scramble::plan_store::FilePlanStore;
use git_scramble::services::strategy::StrategyFactory;

fn main() {
    let cli = Cli::parse();
    let command = cli.into_command();

    let git = Git::new();
    let editor = SystemEditor::new();
    let plan_store = FilePlanStore::new();
    let strategies = StrategyFactory::new();

    let mut app = App::new(git, editor, plan_store, strategies);
    if let Err(err) = app.run(command) {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
}
