![git-reabsorb: Branch-based commit organisational tool. With some AI.](./banner.png)

# ~~git-scramble~~ git-reabsorb

Reorganize git commits by unstaging and recommitting with new structure.

## Building

Requires Rust 1.70 or later.

```bash
# Build
cargo build --release

# Run tests
cargo test

# Install locally
cargo install --path .
```

## Usage

```bash
# Basic usage
git-reabsorb

# Use LLM strategy for intelligent commit reorganization
git-reabsorb --strategy llm

# Skip pre-commit hooks
git-reabsorb --no-verify

# Combine options
git-reabsorb --strategy llm --no-verify
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
