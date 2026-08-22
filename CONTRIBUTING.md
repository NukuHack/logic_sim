# Contributing

Thanks for your interest in contributing. This document outlines the conventions and expectations for code contributions.

## Getting Started

1. Fork the repository and clone it locally.
2. Install Rust via `rustup` and run `build.sh` to ensure everything compiles and to verify all tests pass before making changes.
3. Create a branch with a descriptive name: `feat/short-description` or `fix/bug-name`.

## Code Style

### Indentation and Formatting

Use tabs for indentation, one per nesting level. Run `cargo fmt` before committing
to ensure consistent formatting across the codebase. Configure your editor to
respect the `rustfmt.toml` if one exists in the project root.

Lines are capped at 150 characters, matching `max_width` in `rustfmt.toml`. This
applies to both code and comments, so keep it in mind when writing doc comments.
Note that `use_small_heuristics = "Max"` is enabled, so `cargo fmt` will collapse
short items (structs, functions, match arms) onto fewer lines than you might expect.
Don't fight the formatter; if a diff looks larger than your change, it's probably
just `cargo fmt` doing its job.

Run `cargo clippy -- -D warnings` before committing as well. CI treats clippy
warnings as errors, so a PR with lint warnings will not pass.

> A `check.sh` script is provided in the repo root to run `cargo fmt` and
> `cargo clippy` together, plus soft warnings for files or lines that exceed
> the length guidelines below.

### Variables and Types

Prefer `let` bindings and avoid `let mut` unless mutation is strictly necessary.
Use `const` for all compile-time constants. Type annotations should be explicit
when the type is not immediately obvious to someone reading the code for the first time.
Obvious types from literals or well-known functions can omit annotations.

### Comments and Documentation

File headers must be multiline doc comments limited to 5 lines, each no longer than
150 characters. Describe the module's purpose and core responsibility without version
info or modification dates. Example:

```rust
//! Handles serialization and deserialization of configuration files.
//! Supports TOML and JSON formats with validation and error recovery.
//! Delegates platform-specific paths to the `paths` module.
```

Inline comments must stay under 3 lines, each under 150 characters.
Focus on explaining why a decision was made or what non-obvious logic achieves.
Do not restate what code already clearly expresses through good naming.

Functions and variables should be named descriptively enough to avoid comments.
Keep documentation minimal but present where public API behavior isn't obvious.
Avoid over-documenting internal helpers with clear, single-purpose implementations.

### Error Handling

Use `Result` and the `?` operator for recoverable errors; reserve `panic!`,
`unwrap()`, and `expect()` for cases that indicate a programming bug rather than
an expected failure mode. In library code, prefer `thiserror` for typed, structured
error enums. In binary/application code, `anyhow` is acceptable for error
propagation where callers don't need to match on error variants.

`unwrap()` and `expect()` are permitted in tests and examples without restriction.
Outside of those, prefer `expect("reason")` over bare `unwrap()` so failures are
self-explanatory, and avoid both where the error can reasonably be propagated instead.

### Unsafe Code

`unsafe` blocks require a `// SAFETY:` comment directly above them explaining why
the invariants being relied upon actually hold. Keep `unsafe` blocks as small as
possible and isolate them behind safe wrapper functions where practical. PRs that
introduce new `unsafe` code should call this out explicitly in the PR description.

### What Not to Include

Never embed update logs, version numbers, or modification dates in comments or
file headers. These belong in `CHANGELOG.md` or git tags. Obvious code should not
be commented. Avoid repeating function or variable names in documentation strings.

## Pull Request Process

1. Ensure `cargo fmt` runs and all tests pass.
2. Write or update tests for any behavioral changes.
3. Update relevant documentation if public API surfaces change.
4. Keep commits focused and atomic. Write commit messages in imperative mood.
5. Open a PR against `master` with a clear description of what changed and why.

## Testing Philosophy

New features should include unit tests. Bug fixes should include a regression test
that fails before the fix and passes after. All tests go in `tests/`.
Property-based tests are encouraged for parsing and serialization logic; use
`proptest` for consistency with existing test suites rather than `quickcheck`.

## Continuous Integration

CI runs `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` on
every PR. All three must pass before a PR can be merged. Running them locally
before pushing saves round-trips.

## Dependencies

Adding a new dependency should be justified in the PR description: what it's for,
and why the standard library or an existing dependency isn't sufficient. Prefer
crates with minimal transitive dependency trees and compatible licenses (MIT/Apache-2.0
preferred). For anything non-trivial, open an issue first to discuss the addition.

## Minimum Supported Rust Version

This project targets the Rust version implied by `edition` in
`rustfmt.toml` and the latest stable toolchain at time of contribution. If a
change requires bumping the MSRV, note it explicitly in the PR description.

## Crate Organization

Keep modules focused on a single responsibility. If a file exceeds roughly 500 lines,
consider splitting it into submodules with their own directories.

## Communication

Open an issue before starting large changes to discuss approach and avoid wasted effort.
Please be respectful and constructive in all interactions.

## License

By contributing, you agree that your code will be licensed under the same terms
as the rest of the project. See `LICENSE` for details.
