use std::collections::{BTreeSet, HashMap, HashSet};
use std::process;

use clap::{Parser, ValueEnum};

use git_scramble::diff_parser::parse_diff;
use git_scramble::editor::{Editor, SystemEditor};
use git_scramble::git::{Git, GitOps, PRE_SCRAMBLE_REF};
use git_scramble::models::{Hunk, PlannedCommit, SourceCommit};
use git_scramble::reorganize::llm::ClaudeCliClient;
use git_scramble::reorganize::{
    delete_plan, has_saved_plan, load_plan, save_plan, GroupByFile, LlmReorganizer,
    PreserveOriginal, Reorganizer, SavedPlan, Squash,
};

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

    if cli.reset {
        return run_reset(&git);
    }

    let editor = SystemEditor::new();
    if cli.apply || cli.resume {
        return run_apply(&git, &editor, cli.resume, cli.no_verify);
    }

    run_scramble(&git, &editor, cli)
}

fn run_reset<G: GitOps>(git: &G) -> Result<(), Box<dyn std::error::Error>> {
    if !git.has_pre_scramble_head() {
        return Err("No pre-scramble state found. Nothing to reset.".into());
    }

    let pre_scramble_head = git.get_pre_scramble_head()?;
    println!(
        "Resetting from {} to pre-scramble state {}",
        short_sha(&git.get_head()?),
        short_sha(&pre_scramble_head)
    );

    git.reset_hard(&pre_scramble_head)?;
    git.clear_pre_scramble_head()?;

    println!("Successfully reset to pre-scramble state.");
    println!("The saved ref ({}) has been cleared.", PRE_SCRAMBLE_REF);

    Ok(())
}

fn run_apply<G: GitOps, E: Editor>(
    git: &G,
    editor: &E,
    is_resume: bool,
    no_verify: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut plan = load_plan()?;

    if is_resume {
        if plan.is_complete() {
            println!("Plan is already complete. Nothing to resume.");
            delete_plan()?;
            return Ok(());
        }
        println!(
            "Resuming plan: {}/{} commits already created",
            plan.next_commit_index,
            plan.commits.len()
        );
    } else {
        if plan.next_commit_index > 0 {
            return Err(format!(
                "Plan has {} commits already applied. Use --resume to continue, or delete .git/scramble/plan.json",
                plan.next_commit_index
            ).into());
        }
        println!("Applying saved plan (strategy: {})", plan.strategy);
    }

    if !is_resume && git.get_head()? != plan.base_sha {
        eprintln!(
            "Warning: HEAD ({}) differs from plan's base ({})",
            short_sha(&git.get_head()?),
            short_sha(&plan.base_sha)
        );
    }

    let hunks = plan.get_working_tree_hunks();
    let new_files_to_commits = plan.get_new_files_to_commits();
    let planned_commits = plan.to_planned_commits();
    let commits_to_apply: Vec<_> = if is_resume {
        planned_commits[plan.next_commit_index..].to_vec()
    } else {
        planned_commits
    };

    print_planned_commits(&commits_to_apply, plan.next_commit_index);

    let result = create_commits_with_progress(
        git,
        editor,
        &hunks,
        &commits_to_apply,
        &new_files_to_commits,
        no_verify,
        &mut plan,
    );

    if let Err(e) = result {
        eprintln!("\nError during commit creation: {}", e);
        eprintln!("Progress saved. Use --resume to continue.");
        save_plan(&plan)?;
        return Err(e);
    }

    delete_plan()?;
    println!("\nDone! Created {} commits.", commits_to_apply.len());
    println!("To undo: git-scramble --reset");

    Ok(())
}

fn run_scramble<G: GitOps, E: Editor>(
    git: &G,
    editor: &E,
    cli: Cli,
) -> Result<(), Box<dyn std::error::Error>> {
    if has_saved_plan() {
        eprintln!("Warning: A saved plan exists. Use --apply/--resume or delete .git/scramble/plan.json\n");
    }

    let (base, head) = parse_range(git, cli.range.as_deref(), cli.base.as_deref())?;
    println!("Scrambling {}..{}", short_sha(&base), short_sha(&head));

    if git.has_pre_scramble_head() {
        eprintln!(
            "Warning: Pre-scramble state exists ({}). Use --reset or it will be overwritten.\n",
            short_sha(&git.get_pre_scramble_head()?)
        );
    }

    let source_commits = git.read_commits(&base, &head)?;
    println!("Found {} commits", source_commits.len());

    let (file_to_commits, new_files_to_commits) = build_file_to_commits_map(git, &source_commits)?;

    if cli.dry_run {
        let hunks = read_hunks_from_source_commits(git, &source_commits)?;
        return show_dry_run_plan(&source_commits, &hunks, cli.strategy);
    }

    git.save_pre_scramble_head()?;
    println!("Saved pre-scramble state to {}", PRE_SCRAMBLE_REF);

    println!("Resetting to {}...", short_sha(&base));
    git.reset_to(&base)?;

    let diff_output = git.get_working_tree_diff()?;
    let hunks = parse_diff_with_commit_mapping(&diff_output, &file_to_commits)?;
    println!("Parsed {} hunks", hunks.len());

    let reorganizer = create_reorganizer(cli.strategy);
    let strategy_name = reorganizer.name().to_string();
    println!("Strategy: {}", strategy_name);

    let planned_commits = reorganizer.reorganize(&source_commits, &hunks)?;
    print_planned_commits(&planned_commits, 0);

    let new_hunks = extract_new_hunks(&planned_commits);

    if cli.plan_only {
        let plan = SavedPlan::new(
            strategy_name, base, head, &planned_commits, &hunks, &new_hunks,
            &file_to_commits, &new_files_to_commits,
        );
        let path = save_plan(&plan)?;
        println!("Plan saved to {}", path.display());
        println!("\nTo apply: git-scramble --apply");
        println!("To undo reset: git-scramble --reset");
        return Ok(());
    }

    let mut plan = SavedPlan::new(
        strategy_name, base, head, &planned_commits, &hunks, &new_hunks,
        &file_to_commits, &new_files_to_commits,
    );
    save_plan(&plan)?;

    let result = create_commits_with_progress(
        git, editor, &hunks, &planned_commits, &new_files_to_commits, cli.no_verify, &mut plan,
    );

    if let Err(e) = result {
        eprintln!("\nError: {}", e);
        save_plan(&plan)?;
        eprintln!("Progress saved. Use --resume to continue, or --reset to undo.");
        return Err(e);
    }

    delete_plan()?;
    println!("\nDone! Created {} commits.", planned_commits.len());
    println!("To undo: git-scramble --reset");

    Ok(())
}

fn create_reorganizer(strategy: Strategy) -> Box<dyn Reorganizer> {
    match strategy {
        Strategy::Preserve => Box::new(PreserveOriginal),
        Strategy::ByFile => Box::new(GroupByFile),
        Strategy::Squash => Box::new(Squash),
        Strategy::Llm => Box::new(LlmReorganizer::new(Box::new(ClaudeCliClient::new()))),
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

fn extract_new_hunks(planned_commits: &[PlannedCommit]) -> Vec<Hunk> {
    planned_commits
        .iter()
        .flat_map(|c| &c.changes)
        .filter_map(|change| {
            if let git_scramble::models::PlannedChange::NewHunk(h) = change {
                Some(h.clone())
            } else {
                None
            }
        })
        .collect()
}

type FileCommitMaps = (HashMap<String, Vec<String>>, HashMap<String, Vec<String>>);

fn build_file_to_commits_map<G: GitOps>(
    git: &G,
    source_commits: &[SourceCommit],
) -> Result<FileCommitMaps, Box<dyn std::error::Error>> {
    let mut file_to_commits: HashMap<String, Vec<String>> = HashMap::new();
    let mut new_files_to_commits: HashMap<String, Vec<String>> = HashMap::new();

    for commit in source_commits {
        for file in git.get_files_changed_in_commit(&commit.sha)? {
            file_to_commits.entry(file).or_default().push(commit.sha.clone());
        }
        for file in git.get_new_files_in_commit(&commit.sha)? {
            new_files_to_commits.entry(file).or_default().push(commit.sha.clone());
        }
    }

    Ok((file_to_commits, new_files_to_commits))
}

fn parse_diff_with_commit_mapping(
    diff_output: &str,
    file_to_commits: &HashMap<String, Vec<String>>,
) -> Result<Vec<Hunk>, Box<dyn std::error::Error>> {
    let mut hunks = parse_diff(diff_output, &[], 0)?;
    for hunk in &mut hunks {
        if let Some(commits) = file_to_commits.get(&hunk.file_path.to_string_lossy().to_string()) {
            hunk.likely_source_commits.clone_from(commits);
        }
    }
    Ok(hunks)
}

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

fn show_dry_run_plan(
    source_commits: &[SourceCommit],
    hunks: &[Hunk],
    strategy: Strategy,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Parsed {} hunks", hunks.len());

    let reorganizer = create_reorganizer(strategy);
    println!("Strategy: {}", reorganizer.name());

    let planned_commits = reorganizer.reorganize(source_commits, hunks)?;
    print_planned_commits(&planned_commits, 0);

    println!("--dry-run: no changes made.");
    Ok(())
}

fn parse_range<G: GitOps>(
    git: &G,
    range: Option<&str>,
    base_branch: Option<&str>,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    match (range, base_branch) {
        (Some(r), None) => {
            let parts: Vec<&str> = r.split("..").collect();
            if parts.len() != 2 {
                return Err(format!("Invalid range: {}. Expected 'base..head'", r).into());
            }
            Ok((git.resolve_ref(parts[0])?, git.resolve_ref(parts[1])?))
        }
        (None, Some(branch)) => Ok((git.resolve_ref(branch)?, git.get_head()?)),
        (None, None) => Ok((git.find_branch_base()?, git.get_head()?)),
        (Some(_), Some(_)) => Err("Cannot specify both range and --base".into()),
    }
}

fn create_commits_with_progress<G: GitOps, E: Editor>(
    git: &G,
    editor: &E,
    hunks: &[Hunk],
    planned_commits: &[PlannedCommit],
    new_files_to_commits: &HashMap<String, Vec<String>>,
    no_verify: bool,
    plan: &mut SavedPlan,
) -> Result<(), Box<dyn std::error::Error>> {
    let total = planned_commits.len();
    let start_index = plan.next_commit_index;

    let mut staged_new_files: HashSet<String> = HashSet::new();
    for commit in planned_commits.iter().take(start_index) {
        let source_shas = collect_source_commits(commit, hunks);
        staged_new_files.extend(find_matching_new_files(new_files_to_commits, &source_shas));
    }

    for (i, planned) in planned_commits.iter().enumerate().skip(start_index) {
        println!("Creating commit {}/{}...", i + 1, total);

        let commit_hunks: Vec<Hunk> = planned
            .changes
            .iter()
            .filter_map(|change| change.resolve(hunks).cloned())
            .collect();
        let commit_hunk_refs: Vec<&Hunk> = commit_hunks.iter().collect();

        let source_shas = collect_source_commits(planned, hunks);
        let new_files: Vec<&String> = new_files_to_commits
            .iter()
            .filter(|(f, cs)| !staged_new_files.contains(*f) && cs.iter().any(|c| source_shas.contains(c)))
            .map(|(f, _)| f)
            .collect();

        let help_text = generate_commit_help(&commit_hunk_refs, &new_files);
        let message = editor.edit(&planned.description.long, &help_text)?;

        git.apply_hunks_to_index(&commit_hunk_refs)?;

        if !new_files.is_empty() {
            let paths: Vec<&std::path::Path> = new_files.iter().map(|f| std::path::Path::new(f.as_str())).collect();
            git.stage_files(&paths)?;
            staged_new_files.extend(new_files.iter().map(|f| (*f).clone()));
        }

        let new_sha = git.commit(&message, no_verify)?;
        println!("  Created {}", short_sha(&new_sha));

        plan.mark_commit_created(new_sha);
        save_plan(plan)?;
    }

    Ok(())
}

fn collect_source_commits(planned: &PlannedCommit, hunks: &[Hunk]) -> HashSet<String> {
    planned
        .changes
        .iter()
        .filter_map(|c| c.resolve(hunks))
        .flat_map(|h| h.likely_source_commits.clone())
        .collect()
}

fn find_matching_new_files(
    new_files_to_commits: &HashMap<String, Vec<String>>,
    source_shas: &HashSet<String>,
) -> Vec<String> {
    new_files_to_commits
        .iter()
        .filter(|(_, cs)| cs.iter().any(|c| source_shas.contains(c)))
        .map(|(f, _)| f.clone())
        .collect()
}

fn generate_commit_help(hunks: &[&Hunk], new_files: &[&String]) -> String {
    let files: BTreeSet<_> = hunks.iter().map(|h| &h.file_path).collect();
    let source_commits: BTreeSet<_> = hunks.iter().flat_map(|h| &h.likely_source_commits).collect();

    let mut lines = vec!["Files in this commit:".to_string()];
    lines.extend(files.iter().map(|f| format!("  {}", f.display())));

    if !new_files.is_empty() {
        lines.push(String::new());
        lines.push("New files:".to_string());
        lines.extend(new_files.iter().map(|f| format!("  {}", f)));
    }

    lines.push(String::new());
    lines.push(format!("Total: {} hunks, {} files, {} new", hunks.len(), files.len(), new_files.len()));

    if !source_commits.is_empty() {
        lines.push(String::new());
        lines.push("Source commits:".to_string());
        lines.extend(source_commits.iter().map(|s| format!("  {}", short_sha(s))));
    }

    lines.push(String::new());
    lines.push("Lines starting with '#' ignored. Empty message aborts.".to_string());

    lines.join("\n")
}
