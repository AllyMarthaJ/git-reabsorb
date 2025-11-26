use std::collections::{BTreeSet, HashMap, HashSet};
use std::process;

use clap::{Parser, ValueEnum};

use git_scramble::diff_parser::parse_diff;
use git_scramble::editor::{Editor, SystemEditor};
use git_scramble::git::{Git, GitOps, PRE_SCRAMBLE_REF};
use git_scramble::models::{Hunk, PlannedCommit, SourceCommit};
use git_scramble::reorganize::llm::ClaudeCliClient;
use git_scramble::reorganize::{GroupByFile, LlmReorganizer, PreserveOriginal, Reorganizer, Squash};

/// Truncate a SHA to 8 characters for display
fn short_sha(sha: &str) -> &str {
    &sha[..8.min(sha.len())]
}

#[derive(Parser)]
#[command(name = "git-scramble")]
#[command(about = "Reorganize git commits by unstaging and recommitting")]
#[command(version)]
struct Cli {
    /// Commit range to scramble (default: auto-detect branch base..HEAD)
    /// Examples: main..HEAD, HEAD~5..HEAD, abc123..def456
    #[arg(value_name = "RANGE", conflicts_with_all = ["reset", "base"])]
    range: Option<String>,

    /// Base branch to scramble from (uses tip of branch)
    /// Examples: main, develop, origin/main, feat/my-feature
    #[arg(short, long, conflicts_with_all = ["reset", "range"])]
    base: Option<String>,

    /// Reorganization strategy
    #[arg(short, long, value_enum, default_value = "preserve", conflicts_with = "reset")]
    strategy: Strategy,

    /// Show plan without executing
    #[arg(long, conflicts_with = "reset")]
    dry_run: bool,

    /// Reset to the pre-scramble state (undo the last scramble)
    #[arg(long)]
    reset: bool,

    /// Skip pre-commit and commit-msg hooks
    #[arg(long, conflicts_with = "reset")]
    no_verify: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum Strategy {
    /// Preserve original commit structure
    Preserve,
    /// Group changes by file (one commit per file)
    ByFile,
    /// Squash all changes into a single commit
    Squash,
    /// Use LLM to intelligently reorganize commits
    Llm,
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

    // Handle reset command
    if cli.reset {
        return run_reset(&git);
    }

    // Run the scramble
    let editor = SystemEditor::new();
    run_scramble(&git, &editor, cli)
}

/// Reset to pre-scramble state
fn run_reset<G: GitOps>(git: &G) -> Result<(), Box<dyn std::error::Error>> {
    // Check if we have a saved state
    if !git.has_pre_scramble_head() {
        return Err("No pre-scramble state found. Nothing to reset.".into());
    }

    let pre_scramble_head = git.get_pre_scramble_head()?;
    let current_head = git.get_head()?;

    println!(
        "Resetting from {} to pre-scramble state {}",
        short_sha(&current_head),
        short_sha(&pre_scramble_head)
    );

    // Hard reset to pre-scramble state
    git.reset_hard(&pre_scramble_head)?;

    // Clear the saved state
    git.clear_pre_scramble_head()?;

    println!("Successfully reset to pre-scramble state.");
    println!(
        "The saved pre-scramble ref ({}) has been cleared.",
        PRE_SCRAMBLE_REF
    );

    Ok(())
}

/// Run the scramble operation
fn run_scramble<G: GitOps, E: Editor>(
    git: &G,
    editor: &E,
    cli: Cli,
) -> Result<(), Box<dyn std::error::Error>> {
    // Determine the range
    let (base, head) = parse_range(git, cli.range.as_deref(), cli.base.as_deref())?;
    println!("Scrambling commits from {}..{}", short_sha(&base), short_sha(&head));

    // Check if there's already a saved pre-scramble state
    if git.has_pre_scramble_head() {
        let saved = git.get_pre_scramble_head()?;
        eprintln!("Warning: A pre-scramble state already exists ({})", short_sha(&saved));
        eprintln!("Run 'git-scramble --reset' to restore it, or it will be overwritten.");
        eprintln!();
    }

    // Step 1: Read source commits (for metadata and file-to-commit mapping)
    let source_commits = git.read_commits(&base, &head)?;
    println!("Found {} commits to reorganize", source_commits.len());

    // Step 2: Build file-to-commits mapping (which commits touched which files)
    let (file_to_commits, new_files_to_commits) = build_file_to_commits_map(git, &source_commits)?;

    if cli.dry_run {
        // For dry-run, we need to read hunks from source commits since we won't reset
        let hunks = read_hunks_from_source_commits(git, &source_commits)?;
        show_dry_run_plan(git, &source_commits, &hunks, &cli)?;
        return Ok(());
    }

    // Step 3: Save pre-scramble HEAD before making any changes
    git.save_pre_scramble_head()?;
    println!(
        "Saved pre-scramble state to {} (use --reset to restore)",
        PRE_SCRAMBLE_REF
    );

    // Step 4: Reset to base (unstage everything to working tree)
    println!("Resetting to {}...", short_sha(&base));
    git.reset_to(&base)?;

    // Step 5: Parse hunks from working tree diff (all relative to base)
    let diff_output = git.get_working_tree_diff()?;
    let hunks = parse_diff_with_commit_mapping(&diff_output, &file_to_commits)?;
    println!("Parsed {} hunks from working tree", hunks.len());

    // Step 6: Choose reorganizer and plan
    let reorganizer: Box<dyn Reorganizer> = match cli.strategy {
        Strategy::Preserve => Box::new(PreserveOriginal),
        Strategy::ByFile => Box::new(GroupByFile),
        Strategy::Squash => Box::new(Squash),
        Strategy::Llm => Box::new(LlmReorganizer::new(Box::new(ClaudeCliClient::new()))),
    };

    println!("Using strategy: {}", reorganizer.name());

    // Plan the reorganization
    let planned_commits = reorganizer.reorganize(&source_commits, &hunks)?;
    println!("\nPlanned {} new commits:", planned_commits.len());
    for (i, commit) in planned_commits.iter().enumerate() {
        let change_count = commit.changes.len();
        println!(
            "  {}. \"{}\" ({} changes)",
            i + 1,
            commit.description.short,
            change_count
        );
    }
    println!();

    // Step 7: Create each commit
    let result = create_commits(git, editor, &hunks, &planned_commits, &new_files_to_commits, cli.no_verify);

    if let Err(e) = result {
        eprintln!("\nError during commit creation: {}", e);
        eprintln!("Restoring pre-scramble state...");
        let pre_scramble = git.get_pre_scramble_head()?;
        git.reset_hard(&pre_scramble)?;
        git.clear_pre_scramble_head()?;
        return Err(e);
    }

    println!(
        "\nDone! Successfully created {} commits.",
        planned_commits.len()
    );
    println!("To undo this scramble, run: git-scramble --reset");

    Ok(())
}

/// Build a mapping of file paths to the commits that touched them
/// Returns (file_to_commits, new_files_to_commits)
fn build_file_to_commits_map<G: GitOps>(
    git: &G,
    source_commits: &[SourceCommit],
) -> Result<(HashMap<String, Vec<String>>, HashMap<String, Vec<String>>), Box<dyn std::error::Error>> {
    let mut file_to_commits: HashMap<String, Vec<String>> = HashMap::new();
    let mut new_files_to_commits: HashMap<String, Vec<String>> = HashMap::new();

    for commit in source_commits {
        let files = git.get_files_changed_in_commit(&commit.sha)?;
        for file in files {
            file_to_commits
                .entry(file)
                .or_default()
                .push(commit.sha.clone());
        }

        // Also track newly added files
        let new_files = git.get_new_files_in_commit(&commit.sha)?;
        for file in new_files {
            new_files_to_commits
                .entry(file)
                .or_default()
                .push(commit.sha.clone());
        }
    }

    Ok((file_to_commits, new_files_to_commits))
}

/// Parse diff output and map hunks to their likely source commits
fn parse_diff_with_commit_mapping(
    diff_output: &str,
    file_to_commits: &HashMap<String, Vec<String>>,
) -> Result<Vec<Hunk>, Box<dyn std::error::Error>> {
    // First parse with empty source commits
    let mut hunks = parse_diff(diff_output, &[], 0)?;

    // Then update each hunk with likely source commits based on file path
    for hunk in &mut hunks {
        let file_path_str = hunk.file_path.to_string_lossy().to_string();
        if let Some(commits) = file_to_commits.get(&file_path_str) {
            hunk.likely_source_commits = commits.clone();
        }
    }

    Ok(hunks)
}

/// Read hunks from source commits (used for dry-run before reset)
fn read_hunks_from_source_commits<G: GitOps>(
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

/// Show the dry-run plan without making changes
fn show_dry_run_plan<G: GitOps>(
    _git: &G,
    source_commits: &[SourceCommit],
    hunks: &[Hunk],
    cli: &Cli,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Parsed {} hunks across all commits", hunks.len());

    let reorganizer: Box<dyn Reorganizer> = match cli.strategy {
        Strategy::Preserve => Box::new(PreserveOriginal),
        Strategy::ByFile => Box::new(GroupByFile),
        Strategy::Squash => Box::new(Squash),
        Strategy::Llm => Box::new(LlmReorganizer::new(Box::new(ClaudeCliClient::new()))),
    };

    println!("Using strategy: {}", reorganizer.name());

    let planned_commits = reorganizer.reorganize(source_commits, hunks)?;
    println!("\nPlanned {} new commits:", planned_commits.len());
    for (i, commit) in planned_commits.iter().enumerate() {
        let change_count = commit.changes.len();
        println!(
            "  {}. \"{}\" ({} changes)",
            i + 1,
            commit.description.short,
            change_count
        );
    }

    println!("\n--dry-run specified, not making any changes.");
    Ok(())
}

/// Parse the range argument, base branch, or auto-detect
fn parse_range<G: GitOps>(
    git: &G,
    range: Option<&str>,
    base_branch: Option<&str>,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    match (range, base_branch) {
        // Explicit range specified (e.g., "main..HEAD")
        (Some(r), None) => {
            let parts: Vec<&str> = r.split("..").collect();
            if parts.len() != 2 {
                return Err(
                    format!("Invalid range format: {}. Expected 'base..head'", r).into(),
                );
            }

            let base = git.resolve_ref(parts[0])?;
            let head = git.resolve_ref(parts[1])?;

            Ok((base, head))
        }
        // Base branch specified (e.g., "--base develop")
        // Use the tip of the branch directly, not merge-base
        (None, Some(branch)) => {
            let base = git.resolve_ref(branch)?;
            let head = git.get_head()?;
            Ok((base, head))
        }
        // Auto-detect: find merge-base with main/master
        (None, None) => {
            let base = git.find_branch_base()?;
            let head = git.get_head()?;
            Ok((base, head))
        }
        // Both specified - shouldn't happen due to clap conflicts_with
        (Some(_), Some(_)) => Err("Cannot specify both range and --base".into()),
    }
}

/// Create all planned commits by applying changes
fn create_commits<G: GitOps, E: Editor>(
    git: &G,
    editor: &E,
    hunks: &[Hunk],
    planned_commits: &[PlannedCommit],
    new_files_to_commits: &HashMap<String, Vec<String>>,
    no_verify: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let total = planned_commits.len();

    // Track which new files have been staged to avoid duplicates
    let mut staged_new_files: HashSet<String> = HashSet::new();

    for (i, planned) in planned_commits.iter().enumerate() {
        println!("Creating commit {}/{}...", i + 1, total);

        // Resolve all changes to concrete hunks
        let commit_hunks: Vec<Hunk> = planned
            .changes
            .iter()
            .filter_map(|change| change.resolve(hunks).cloned())
            .collect();

        let commit_hunk_refs: Vec<&Hunk> = commit_hunks.iter().collect();

        // Collect all source commits that contributed to this planned commit's hunks
        let source_commits: HashSet<&String> = commit_hunks
            .iter()
            .flat_map(|h| &h.likely_source_commits)
            .collect();

        // Find new files that belong to this commit (their source commits overlap)
        let new_files_for_commit: Vec<&String> = new_files_to_commits
            .iter()
            .filter(|(file, commits)| {
                !staged_new_files.contains(*file)
                    && commits.iter().any(|c| source_commits.contains(c))
            })
            .map(|(file, _)| file)
            .collect();

        // Generate help text showing what's in this commit
        let help_text = generate_commit_help(&commit_hunk_refs, &new_files_for_commit);

        // Open editor for commit message
        let message = editor.edit(&planned.description.long, &help_text)?;

        // Stage all hunks for this commit (grouped by file)
        git.apply_hunks_to_index(&commit_hunk_refs)?;

        // Stage new files for this commit
        if !new_files_for_commit.is_empty() {
            let paths: Vec<&std::path::Path> = new_files_for_commit
                .iter()
                .map(|f| std::path::Path::new(f.as_str()))
                .collect();
            git.stage_files(&paths)?;

            // Mark these files as staged
            for file in &new_files_for_commit {
                staged_new_files.insert((*file).clone());
            }
        }

        // Create the commit
        let new_sha = git.commit(&message, no_verify)?;
        println!("  Created commit {}", short_sha(&new_sha));
    }

    Ok(())
}

/// Generate help text for the commit editor
fn generate_commit_help(hunks: &[&Hunk], new_files: &[&String]) -> String {
    let mut lines = Vec::new();
    lines.push("Files in this commit:".to_string());

    // Collect unique file paths from hunks
    let files: BTreeSet<_> = hunks.iter().map(|h| &h.file_path).collect();
    for file in &files {
        lines.push(format!("  {}", file.display()));
    }

    // Show new files
    if !new_files.is_empty() {
        lines.push(String::new());
        lines.push("New files:".to_string());
        for file in new_files {
            lines.push(format!("  {}", file));
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "Total: {} hunks across {} files, {} new files",
        hunks.len(),
        files.len(),
        new_files.len()
    ));

    // Show likely source commits for context
    let all_source_commits: BTreeSet<_> = hunks
        .iter()
        .flat_map(|h| &h.likely_source_commits)
        .collect();
    if !all_source_commits.is_empty() {
        lines.push(String::new());
        lines.push("Likely source commits:".to_string());
        for sha in all_source_commits {
            lines.push(format!("  {}", short_sha(sha)));
        }
    }

    lines.push(String::new());
    lines.push("Lines starting with '#' will be ignored.".to_string());
    lines.push("An empty message aborts the commit.".to_string());

    lines.join("\n")
}
