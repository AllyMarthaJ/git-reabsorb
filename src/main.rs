use std::process;

use clap::{Parser, ValueEnum};

use git_scramble::editor::{Editor, SystemEditor};
use git_scramble::git::{Git, GitOps};
use git_scramble::models::{Hunk, PlannedCommit, SourceCommit};
use git_scramble::reorganize::{GroupByFile, PreserveOriginal, Reorganizer, Squash};

#[derive(Parser)]
#[command(name = "git-scramble")]
#[command(about = "Reorganize git commits by unstaging and recommitting")]
#[command(version)]
struct Cli {
    /// Commit range to scramble (default: auto-detect branch base..HEAD)
    /// Examples: main..HEAD, HEAD~5..HEAD, abc123..def456
    #[arg(value_name = "RANGE")]
    range: Option<String>,

    /// Reorganization strategy
    #[arg(short, long, value_enum, default_value = "preserve")]
    strategy: Strategy,

    /// Show plan without executing
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Strategy {
    /// Preserve original commit structure
    Preserve,
    /// Group changes by file (one commit per file)
    ByFile,
    /// Squash all changes into a single commit
    Squash,
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let git = Git::new();
    let editor = SystemEditor::new();

    // Determine the range
    let (base, head) = parse_range(&git, cli.range.as_deref())?;
    println!("Scrambling commits from {}..{}", &base[..8.min(base.len())], &head[..8.min(head.len())]);

    // Save original HEAD for potential abort
    let original_head = git.get_head()?;

    // Read commits and hunks
    let source_commits = git.read_commits(&base, &head)?;
    println!(
        "Found {} commits to reorganize",
        source_commits.len()
    );

    let hunks = read_all_hunks(&git, &source_commits)?;
    println!("Parsed {} hunks across all commits", hunks.len());

    // Choose reorganizer
    let reorganizer: Box<dyn Reorganizer> = match cli.strategy {
        Strategy::Preserve => Box::new(PreserveOriginal),
        Strategy::ByFile => Box::new(GroupByFile),
        Strategy::Squash => Box::new(Squash),
    };

    println!("Using strategy: {}", reorganizer.name());

    // Plan the reorganization
    let planned_commits = reorganizer.reorganize(&source_commits, &hunks)?;
    println!("\nPlanned {} new commits:", planned_commits.len());
    for (i, commit) in planned_commits.iter().enumerate() {
        let hunk_count = commit.hunk_ids.len();
        println!(
            "  {}. \"{}\" ({} hunks)",
            i + 1,
            commit.description.short,
            hunk_count
        );
    }

    if cli.dry_run {
        println!("\n--dry-run specified, not making any changes.");
        return Ok(());
    }

    println!();

    // Reset to base
    println!("Resetting to {}...", &base[..8.min(base.len())]);
    git.reset_to(&base)?;

    // Create each commit
    let result = create_commits(&git, &editor, &hunks, &planned_commits);

    if let Err(e) = result {
        eprintln!("\nError during commit creation: {}", e);
        eprintln!("Restoring original state...");
        git.reset_hard(&original_head)?;
        return Err(e);
    }

    println!("\nDone! Successfully created {} commits.", planned_commits.len());
    Ok(())
}

/// Parse the range argument or auto-detect
fn parse_range<G: GitOps>(git: &G, range: Option<&str>) -> Result<(String, String), Box<dyn std::error::Error>> {
    match range {
        Some(r) => {
            // Parse "base..head" format
            let parts: Vec<&str> = r.split("..").collect();
            if parts.len() != 2 {
                return Err(format!("Invalid range format: {}. Expected 'base..head'", r).into());
            }

            // Resolve refs to SHAs
            let base = git.resolve_ref(parts[0])?;
            let head = git.resolve_ref(parts[1])?;

            Ok((base, head))
        }
        None => {
            // Auto-detect: find merge-base with main/master
            let base = git.find_branch_base()?;
            let head = git.get_head()?;
            Ok((base, head))
        }
    }
}

/// Read hunks from all source commits
fn read_all_hunks<G: GitOps>(
    git: &G,
    source_commits: &[SourceCommit],
) -> Result<Vec<Hunk>, Box<dyn std::error::Error>> {
    let mut all_hunks = Vec::new();
    let mut hunk_id = 0;

    for commit in source_commits {
        let hunks = git.read_hunks(&commit.sha, hunk_id)?;
        hunk_id += hunks.len();
        all_hunks.extend(hunks);
    }

    Ok(all_hunks)
}

/// Create all planned commits
fn create_commits<G: GitOps, E: Editor>(
    git: &G,
    editor: &E,
    hunks: &[Hunk],
    planned_commits: &[PlannedCommit],
) -> Result<(), Box<dyn std::error::Error>> {
    let total = planned_commits.len();

    for (i, planned) in planned_commits.iter().enumerate() {
        println!("Creating commit {}/{}...", i + 1, total);

        // Collect hunks for this commit
        let commit_hunks: Vec<&Hunk> = planned
            .hunk_ids
            .iter()
            .filter_map(|id| hunks.iter().find(|h| h.id == *id))
            .collect();

        // Generate help text showing what's in this commit
        let help_text = generate_commit_help(&commit_hunks);

        // Open editor for commit message
        let message = editor.edit(&planned.description.long, &help_text)?;

        // Stage the hunks
        git.apply_hunks_to_index(&commit_hunks)?;

        // Create the commit
        let new_sha = git.commit(&message)?;
        println!("  Created commit {}", &new_sha[..8.min(new_sha.len())]);
    }

    Ok(())
}

/// Generate help text for the commit editor
fn generate_commit_help(hunks: &[&Hunk]) -> String {
    use std::collections::BTreeSet;

    let mut lines = Vec::new();
    lines.push("Files in this commit:".to_string());

    // Collect unique file paths
    let files: BTreeSet<_> = hunks.iter().map(|h| &h.file_path).collect();
    for file in &files {
        lines.push(format!("  {}", file.display()));
    }

    lines.push(String::new());
    lines.push(format!("Total: {} hunks across {} files", hunks.len(), files.len()));
    lines.push(String::new());
    lines.push("Lines starting with '#' will be ignored.".to_string());
    lines.push("An empty message aborts the commit.".to_string());

    lines.join("\n")
}
