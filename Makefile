.PHONY: all help build build-release test test-doc nextest mutants check fmt fmt-fix clippy lint lint-fix coverage coverage-ci coverage-check coverage-serve clean setup install-tools watch bench doc outdated unused-deps audit

# Ensure cargo and tools are on PATH
export PATH := $(HOME)/.cargo/bin:$(PATH)

# Default target: every check the CI workflow runs. Green here = green in CI.
# (coverage-check runs the full test suite internally with instrumentation)
all: fmt clippy coverage-check test-doc build-release

help:
	@echo "Available targets:"
	@echo "  all            Run every CI check: fmt, clippy, coverage, doctests, release build"
	@echo "  build          Build the project (debug)"
	@echo "  build-release  Build the release binary"
	@echo "  test           Run all tests"
	@echo "  test-doc       Run doctests"
	@echo "  nextest        Run tests with cargo-nextest"
	@echo "  mutants        Run mutation testing with cargo-mutants"
	@echo "  check          Run cargo check"
	@echo "  fmt            Check code formatting"
	@echo "  fmt-fix        Format the code"
	@echo "  clippy         Run the Clippy linter"
	@echo "  lint           Run formatting check and Clippy"
	@echo "  lint-fix       Run Clippy with auto-fix"
	@echo "  coverage       Generate code coverage report (HTML)"
	@echo "  coverage-check Check coverage meets the 90% line floor"
	@echo "  coverage-ci    Generate coverage report for CI (LCOV)"
	@echo "  coverage-serve Generate and serve coverage report on localhost:8080"
	@echo "  doc            Build API documentation"
	@echo "  audit          Run dependency security audit"
	@echo "  unused-deps    Check for unused dependencies"
	@echo "  outdated       Check for outdated dependencies"
	@echo "  clean          Remove build artifacts"
	@echo "  setup          Install tools and configure git hooks (run once after clone)"
	@echo "  install-tools  Install required development tools"

# Build the project
build:
	cargo build

# Build release version
build-release:
	cargo build --release

# Run all tests
test:
	cargo test --all-targets --all-features

# Run doctests (not covered by --all-targets)
test-doc:
	cargo test --doc --all-features

# Run tests with cargo-nextest (faster, process-per-test)
nextest:
	@command -v cargo-nextest >/dev/null 2>&1 || { echo "cargo-nextest not installed. Run 'make install-tools' first."; exit 1; }
	cargo nextest run --all-targets --all-features

# Run mutation testing with cargo-mutants (uses nextest via .cargo/mutants.toml)
mutants:
	@command -v cargo-mutants >/dev/null 2>&1 || { echo "cargo-mutants not installed. Run 'make install-tools' first."; exit 1; }
	cargo mutants

# Run cargo check
check:
	cargo check --all-targets --all-features

# Check formatting without modifying files
fmt:
	cargo fmt --all -- --check

# Format code
fmt-fix:
	cargo fmt --all

# Run the Clippy linter; every warning is an error
clippy:
	cargo clippy --all-targets --all-features -- -D warnings

# Fast pre-commit loop: formatting check plus Clippy, no tests
lint: fmt clippy

# Run clippy with auto-fix
lint-fix:
	cargo clippy --all-targets --all-features --fix --allow-dirty --allow-staged

# Generate code coverage report (using cargo-llvm-cov)
coverage:
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { echo "cargo-llvm-cov not installed. Run 'make install-tools' first."; exit 1; }
	cargo llvm-cov --html --output-dir coverage --all-targets --all-features
	@echo "Coverage report generated in coverage/html/index.html"

# Generate LCOV coverage for CI
coverage-ci:
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { echo "cargo-llvm-cov not installed. Run 'make install-tools' first."; exit 1; }
	cargo llvm-cov --lcov --output-path coverage/lcov.info --all-targets --all-features

# Check coverage meets 90% threshold (runs tests internally)
coverage-check:
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { echo "cargo-llvm-cov not installed. Run 'make install-tools' first."; exit 1; }
	cargo llvm-cov --fail-under-lines 90 --all-targets --all-features

# Generate and serve coverage report
coverage-serve: coverage
	@echo "Serving coverage report at http://localhost:8080"
	python3 -m http.server 8080 -d coverage/html

# Clean build artifacts
clean:
	cargo clean
	rm -rf coverage/

# Install development tools
setup: install-tools
	git config core.hooksPath hooks/
	@echo "Git hooks configured."

install-tools:
	@echo "Installing Rust development tools..."
	rustup component add rustfmt clippy rust-src rust-analyzer
	@echo "Installing cargo-llvm-cov for code coverage..."
	cargo install cargo-llvm-cov
	@echo "Installing cargo-nextest for faster test execution..."
	cargo install cargo-nextest --locked
	@echo "Installing cargo-mutants for mutation testing..."
	cargo install cargo-mutants
	@echo "Installing cargo-machete for unused dependency detection..."
	cargo install cargo-machete
	@echo "Installing cargo-audit for dependency security auditing..."
	cargo install cargo-audit
	@echo "All tools installed successfully!"

# Watch for changes and run tests
watch:
	@command -v cargo-watch >/dev/null 2>&1 || { echo "cargo-watch not installed. Install with: cargo install cargo-watch"; exit 1; }
	cargo watch -x 'test --all-targets --all-features'

# Run benchmarks (if any)
bench:
	cargo bench

# Build API documentation
doc:
	cargo doc --no-deps --all-features

# Check for outdated dependencies
outdated:
	@command -v cargo-outdated >/dev/null 2>&1 || { echo "cargo-outdated not installed. Install with: cargo install cargo-outdated"; exit 1; }
	cargo outdated

# Check for unused dependencies
unused-deps:
	@command -v cargo-machete >/dev/null 2>&1 || { echo "cargo-machete not installed. Run 'make install-tools' first."; exit 1; }
	cargo machete

# Run dependency security audit (RustSec advisory database)
audit:
	@command -v cargo-audit >/dev/null 2>&1 || { echo "cargo-audit not installed. Run 'make install-tools' first."; exit 1; }
	cargo audit
