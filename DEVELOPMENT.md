# Development Guide

This document provides information about the development workflow, tooling, and best practices for this project.

## Prerequisites

- Rust 1.88.0 or later
- Cargo (comes with Rust)
- Git

## Quick Start

### Install Development Tools

```bash
make install-tools
```

This will install:
- `rustfmt` - Code formatter
- `clippy` - Linter
- `rust-src` - Rust source code (for IDE support)
- `rust-analyzer` - Language server
- `cargo-llvm-cov` - Code coverage tool (cross-platform)
- `cargo-nextest` - Fast test runner (process-per-test)
- `cargo-mutants` - Mutation testing tool

### Build the Project

```bash
make build
```

### Run Tests

```bash
make test
```

### Format Code

```bash
make fmt
```

### Run Linter

```bash
make lint
```

## Development Workflow

### Before Committing

Run all CI checks locally:

```bash
make ci
```

This will:
1. Check code formatting
2. Run clippy linter
3. Run all tests

### Code Formatting

We use `rustfmt` with custom configuration (see `rustfmt.toml`). Format your code before committing:

```bash
make fmt
```

To check if code is properly formatted without modifying files:

```bash
make fmt-check
```

### Linting

We use `clippy` with strict settings (see `.clippy.toml`). Run the linter:

```bash
make lint
```

To automatically fix some issues:

```bash
make lint-fix
```

### Testing

Run all tests:

```bash
make test
```

Run tests with cargo-nextest (faster, process-per-test execution):

```bash
make nextest
```

Run tests with output:

```bash
cargo test -- --nocapture
```

Run specific test:

```bash
cargo test test_name
```

### Mutation Testing

Run mutation testing to find gaps in test coverage:

```bash
make mutants
```

This uses [cargo-mutants](https://mutants.rs/) configured in `.cargo/mutants.toml` to inject bugs and verify tests catch them. It runs with nextest as the test runner for faster execution. See the [nextest integration docs](https://nexte.st/docs/integrations/cargo-mutants/) for details.

### Dependency Auditing
```bash
```
### Code Coverage

Generate a code coverage report:

```bash
make coverage
```

This will create an HTML report in `coverage/html/index.html`. The project aims for 91% line coverage (configured in `Cargo.toml`).

For CI/CD pipelines, generate LCOV format:

```bash
make coverage-ci
```

This creates `coverage/lcov.info` for integration with coverage reporting tools.

## Project Structure

```
.
├── src/
│   ├── application_specification/  # Application spec parsing and validation
│   ├── lib/                        # Core library modules
│   │   ├── deployment_specification/
│   │   ├── installer/
│   │   ├── runtime/
│   │   └── system/
│   ├── pipeline/                   # Pipeline traits and implementations
│   ├── lib.rs                      # Library root
│   └── main.rs                     # Binary entry point
├── tests/                          # Integration tests
└── Cargo.toml                      # Package manifest
```

## Configuration Files

- `Cargo.toml` - Package manifest and dependencies
- `rustfmt.toml` - Code formatting rules
- `.clippy.toml` - Linter configuration
- `.editorconfig` - Editor consistency settings
- `.cargo/config.toml` - Cargo configuration and aliases
- `.cargo/mutants.toml` - Mutation testing configuration
- `.config/nextest.toml` - Nextest test runner configuration
- `rust-toolchain.toml` - Rust toolchain version (gitignored, local only)

## Cargo Aliases

The following aliases are configured in `.cargo/config.toml`:

```bash
cargo fmt-check      # Check formatting
cargo lint           # Run clippy
cargo lint-all       # Run clippy on all targets
cargo test-all       # Run all tests
cargo coverage       # Generate HTML coverage report
cargo coverage-ci    # Generate XML coverage for CI
cargo check-all      # Check all targets
```

## Best Practices

### Code Style

- Follow Rust naming conventions
- Keep functions focused and under 100 lines
- Write descriptive variable names
- Add comments for complex logic
- Document public APIs with doc comments

### Testing

- Write unit tests for all public functions
- Add integration tests for end-to-end scenarios
- Use descriptive test names
- Aim for 91% code coverage
- Test error cases and edge conditions

### Error Handling

- Use `Result` and `Option` types appropriately
- Create custom error types with `thiserror`
- Provide meaningful error messages
- Don't use `unwrap()` or `expect()` in production code

### Dependencies

- Keep dependencies minimal
- Review licenses before adding dependencies
- Pin versions for stability
- Update dependencies regularly

## Troubleshooting

### Build Issues

If you encounter build issues, try:

```bash
make clean
cargo update
make build
```

### Test Failures

Run tests with verbose output:

```bash
cargo test -- --nocapture --test-threads=1
```

### Coverage Tool Issues

If `cargo-llvm-cov` fails, ensure you have the latest version:

```bash
cargo install cargo-llvm-cov --force
```

## Additional Tools

### cargo-watch (Optional)

For automatic rebuilds on file changes:

```bash
cargo install cargo-watch
make watch
```

### cargo-outdated (Optional)

To check for outdated dependencies:

```bash
cargo install cargo-outdated
make outdated
```

## Resources

- [Rust Book](https://doc.rust-lang.org/book/)
- [Rust by Example](https://doc.rust-lang.org/rust-by-example/)
- [Cargo Book](https://doc.rust-lang.org/cargo/)
- [Clippy Lints](https://rust-lang.github.io/rust-clippy/master/)
- [Rustfmt Configuration](https://rust-lang.github.io/rustfmt/)
