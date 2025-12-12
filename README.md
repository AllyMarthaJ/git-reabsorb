![git-reabsorb: Branch-based commit organisational tool. With some AI.](./banner.png)

# ~~git-scramble~~ git-reabsorb

Ever make a mess of your commits while working on a feature branch? This tool helps you reorganize them before merge.

It works by soft-resetting your branch to a base commit, then recommitting your changes using a strategy you choose—whether that's preserving the original structure, grouping by file, squashing everything, or letting an LLM figure out a sensible organization.

## Installation

```bash
cargo install --path .
```

Or build from source:

```bash
cargo build --release
```

## Quick Start

```bash
# See what it would do (dry run)
git-reabsorb -n

# Reorganize commits on current branch
git-reabsorb

# Let an LLM organize your commits intelligently
git-reabsorb -s llm

# Specify a base branch explicitly
git-reabsorb --base main
```

## Strategies

| Strategy | Flag | What it does |
|----------|------|--------------|
| `preserve` | `-s preserve` | Keep original commit structure (default) |
| `by-file` | `-s by-file` | One commit per file |
| `squash` | `-s squash` | Everything in one commit |
| `llm` | `-s llm` | AI-powered reorganization |
| `hierarchical` | `-s hierarchical` | Multi-phase LLM for large changes |

## Useful tips

### Plan and Apply Separately

```bash
# Generate and save a plan (doesn't modify your branch)
git-reabsorb --save-plan

# Review the plan, then apply when ready
git-reabsorb apply
```

### Undo

```bash
# Reset to pre-reabsorb state
git-reabsorb reset
```

### Assess Commit Quality

```bash
# Score your commits against quality criteria
git-reabsorb assess

# Compare before/after
git-reabsorb assess --save before.json
git-reabsorb
git-reabsorb assess --save after.json
git-reabsorb compare before.json after.json
```

## LLM Configuration

For the `llm` and `hierarchical` strategies, configure your provider:

```bash
# Use Claude (default, requires claude CLI)
git-reabsorb -s llm

# Use a local model via OpenCode
git-reabsorb -s llm --llm-provider opencode --opencode-backend lmstudio

# Or via environment
export GIT_REABSORB_LLM_PROVIDER=claude
export GIT_REABSORB_LLM_MODEL=claude-sonnet-4-20250514
```

As of writing, we default to `claude` when no provider is specified for the best performance.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.
