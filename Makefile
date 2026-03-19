.PHONY: help build test check fmt lint clean coverage install-tools nextest mutants setup coverage-serve unused-deps

# Ensure cargo and tools are on PATH
export PATH := $(HOME)/.cargo/bin:$(PATH)

# Default target
help:
	@echo "Available targets:"
	@echo "  make build          - Build the project"
	@echo "  make setup          - Install tools and configure git hooks (run once after clone)"
	@echo "  make test           - Run all tests"
	@echo "  make nextest        - Run tests with cargo-nextest"
	@echo "  make mutants        - Run mutation testing with cargo-mutants"
	@echo "  make check          - Run cargo check"
	@echo "  make fmt            - Format code with rustfmt"
	@echo "  make fmt-check      - Check code formatting"
	@echo "  make lint           - Run clippy linter"
	@echo "  make lint-fix       - Run clippy with auto-fix"
	@echo "  make coverage       - Generate code coverage report (HTML)"
	@echo "  make coverage-serve - Generate and serve coverage report on localhost:8080"
	@echo "  make coverage-ci    - Generate coverage report for CI (LCOV)"
	@echo "  make unused-deps    - Check for unused dependencies"
	@echo "  make clean          - Clean build artifacts"
	@echo "  make install-tools  - Install required development tools"
	@echo "  make ci             - Run all CI checks (fmt, lint, test)"

# Build the project
build:
	cargo build

# Build release version
build-release:
	cargo build --release

# Run all tests
test:
	cargo test --all-targets --all-features

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

# Format code
fmt:
	cargo fmt --all

# Check formatting without modifying files
fmt-check:
	cargo fmt --all -- --check

# Run clippy linter
lint:
	cargo clippy --all-targets --all-features -- -D warnings

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
	@echo "All tools installed successfully!"

# Run all CI checks (coverage-check runs tests internally with instrumentation)
ci: fmt-check lint coverage-check
	@echo "All CI checks passed!"

# Check coverage meets 91% threshold (runs tests internally)
coverage-check:
	@command -v cargo-llvm-cov >/dev/null 2>&1 || { echo "cargo-llvm-cov not installed. Run 'make install-tools' first."; exit 1; }
	cargo llvm-cov --fail-under-lines 91 --all-targets --all-features

# Watch for changes and run tests
watch:
	@command -v cargo-watch >/dev/null 2>&1 || { echo "cargo-watch not installed. Install with: cargo install cargo-watch"; exit 1; }
	cargo watch -x 'test --all-targets --all-features'

# Run benchmarks (if any)
bench:
	cargo bench

# Generate and open documentation
doc:
	cargo doc --open --all-features

# Check for outdated dependencies
outdated:
	@command -v cargo-outdated >/dev/null 2>&1 || { echo "cargo-outdated not installed. Install with: cargo install cargo-outdated"; exit 1; }
	cargo outdated

# Generate and serve coverage report
coverage-serve: coverage
	@echo "Serving coverage report at http://localhost:8080"
	python3 -m http.server 8080 -d coverage/html

# Check for unused dependencies
unused-deps:
	@command -v cargo-machete >/dev/null 2>&1 || { echo "cargo-machete not installed. Run 'make install-tools' first."; exit 1; }
	cargo machete
