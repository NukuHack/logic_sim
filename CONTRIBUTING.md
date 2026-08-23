# Contributing

Thanks for your interest in contributing. This document outlines the conventions and expectations for code contributions.

## Getting Started

1. Fork the repository and clone it locally.
2. Install Rust via [rustup](https://rustup.rs).
3. Run `build.sh -y` once before making changes to confirm everything compiles and all tests pass on a clean tree.
4. Create a branch with a descriptive name: `feat/short-description` or `fix/bug-name`.

## Code Style

### Formatting and Linting

Formatting is enforced by `cargo fmt` and linting by `cargo clippy`. All style
parameters — indentation, line width, import ordering, and size heuristics — are
defined in `rustfmt.toml`; consult it rather than assuming fixed limits. Configure
your editor to respect that configuration so formatting is applied as you type.

Don't fight the formatter; if a diff looks larger than your change, it's probably
just `cargo fmt` doing its job.

> A `build.sh -y` script is provided in the repo root to run formatting, linting,
> and tests together.

### Variables and Types

Prefer immutable bindings; only introduce mutability where it's strictly necessary.
Use `const` for compile-time constants. Add explicit type annotations where the
type isn't immediately obvious to a first-time reader; omit them where literals
or well-known functions make the type self-evident.

### Comments and Documentation

Every source file starts with a short multiline doc comment (`//!`) describing the
module's purpose and core responsibility — no version info or modification dates.
Example:

```rust
//! Handles serialization and deserialization of configuration files.
//! Supports TOML and JSON formats with validation and error recovery.
//! Delegates platform-specific paths to the `paths` module.
```

Inline comments should be brief and focus on *why* a decision was made, not *what*
the code does. Do not restate what good naming already expresses. Prefer descriptive
function and variable names over explanatory comments, and avoid documenting internal
helpers whose implementation is clear and single-purpose.

### Error Handling

Use `Result` and the `?` operator for recoverable errors; reserve `panic!`,
`unwrap()`, and `expect()` for cases that indicate a programming bug rather than an
expected failure mode. In library code, prefer `thiserror` for typed, structured
error enums. In binary/application code, `anyhow` is acceptable where callers don't
need to match on error variants.

`unwrap()` and `expect()` are permitted freely in tests and examples. Elsewhere,
prefer `expect("reason")` over bare `unwrap()`, and avoid both where the error can
reasonably be propagated instead.

### Unsafe Code

`unsafe` blocks require a `// SAFETY:` comment directly above them explaining why
the invariants being relied upon actually hold. Keep unsafe blocks as small as
possible and isolate them behind safe wrappers where practical. PRs introducing new
`unsafe` code must call this out explicitly in the description.

### What Not to Include

Never embed update logs, version numbers, or modification dates in comments or file
headers — these belong in git tags or release notes. Don't comment obvious code or
repeat function and variable names in doc strings.

## Testing

Unit tests live inline in `#[cfg(test)]` modules next to the code they cover.
Integration tests go in `tests/`. New features should include unit tests; bug fixes
must include a regression test that fails before the fix and passes after.

Property-based testing is encouraged for parsing and serialization logic; if you add
such tests, prefer `proptest`.

## Pull Request Process

1. Run `./build.sh -y` and make sure all checks and tests pass.
2. Write or update tests for any behavioral changes.
3. Update documentation if public API surfaces change.
4. Keep commits focused and atomic; write commit messages in imperative mood.
5. Open a PR against `master` describing what changed and why.

## Dependencies

New dependencies must be justified in the PR description: what they're for and why
the standard library or an existing dependency isn't sufficient. Prefer crates with
minimal transitive dependency trees and permissive licenses (MIT/Apache-2.0
preferred). For anything non-trivial, open an issue first to discuss.

## Toolchain

This project tracks the latest stable Rust and uses the edition declared in
`Cargo.toml`. If a change requires bumping either, note it explicitly in the PR
description.

## Crate Organization

Keep modules focused on a single responsibility. If a file grows past the size the
project considers reasonable (enforced as a warning by `build.sh`), split it into
submodules.

## Communication

Open an issue before starting large changes to discuss approach and avoid wasted
effort. Please be respectful and constructive in all interactions.

## License

By contributing, you agree that your code will be licensed under the same terms as
the rest of the project. See `LICENSE` for details.
