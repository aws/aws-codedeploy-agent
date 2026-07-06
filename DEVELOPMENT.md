# Development Guide

This document provides information about the development workflow, tooling, and best practices for this project.

## Prerequisites

- Rust (the minimum supported version is declared as `rust-version` in `Cargo.toml`)
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

Scan dependencies against the [RustSec advisory database](https://rustsec.org/) for known vulnerabilities:

```bash
make audit
```

This uses [cargo-audit](https://github.com/rustsec/rustsec/tree/main/cargo-audit) to check `Cargo.lock` for crates with reported security advisories.

### Code Coverage

Generate a code coverage report:

```bash
make coverage
```

This will create an HTML report in `coverage/html/index.html`. The project aims for 95% line coverage.

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
- Aim for 95% code coverage
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

## Building on Windows

The agent supports Windows via the `x86_64-pc-windows-gnu` or
`x86_64-pc-windows-msvc` targets.

### Option A: MSVC (recommended for local development)

Requires Visual Studio 2022 Build Tools with the C++ workload and vcpkg:

```powershell
# Install OpenSSL via vcpkg
vcpkg install openssl:x64-windows-static

# Set environment
$env:OPENSSL_STATIC = "1"
$env:OPENSSL_DIR = "C:\vcpkg\installed\x64-windows-static"

# Build
cargo build --release
```

### Option B: MinGW/MSYS2 (used by CI)

```powershell
# Install MSYS2 (https://www.msys2.org), then in an MSYS2 terminal:
pacman -S mingw-w64-x86_64-gcc mingw-w64-x86_64-openssl mingw-w64-x86_64-pkg-config

# Set environment (PowerShell)
$env:PATH = "C:\msys64\mingw64\bin;$env:USERPROFILE\.cargo\bin;$env:PATH"
$env:OPENSSL_DIR = "C:\msys64\mingw64"
$env:OPENSSL_STATIC = "1"

# Use the GNU toolchain
rustup default stable-x86_64-pc-windows-gnu

# Build
cargo build --release
```

## Running the Agent Locally

Build and locate the binary:

```bash
cargo build --release
AGENT=target/release/codedeploy-agent
```

The agent auto-creates its PID, state, and log directories on startup. Default
paths live under `/opt/codedeploy-agent/` (owned by root in production). For
local dev, make them writable once:

```bash
sudo mkdir -p /opt/codedeploy-agent
sudo chown -R "$(whoami)" /opt/codedeploy-agent
```

### Lifecycle commands

```bash
$AGENT start      # Start the daemon (master + worker)
$AGENT status     # Check if running (exit 0 = running, exit 3 = stopped)
$AGENT stop       # Graceful shutdown
$AGENT restart    # Stop then start
```

### Quick smoke test

```bash
cargo build --release
AGENT=target/release/codedeploy-agent

# Start with a region (required — no IMDS locally)
AWS_REGION=us-east-1 $AGENT start &
sleep 3
$AGENT status                          # "running", exit 0
ps aux | grep codedeploy-agent | grep -v grep # master (start) + worker

# Check logs
cat /opt/codedeploy-agent/logs/codedeploy-agent*

# Stop and verify
$AGENT stop
$AGENT status                          # "not running", exit 3
```

### Using a config file

Pass `--config-file` to override defaults. A sample config lives at
`conf/codedeployagent.yml`.

Create a dev config for faster iteration:

```bash
cat > /tmp/codedeploy-dev.yml << 'EOF'
verbose: true
wait_between_runs: 5
log_dir: /opt/codedeploy-agent/logs
EOF

AWS_REGION=us-east-1 $AGENT --config-file /tmp/codedeploy-dev.yml start &
```

### Running the worker directly (no daemon)

For quick debugging, run the worker subprocess directly (no master, no PID
file):

```bash
# Runs in foreground, Ctrl-C to stop
AWS_REGION=us-east-1 $AGENT --config-file /tmp/codedeploy-dev.yml worker
```

### Testable configuration options

All options can be set in the YAML config file. See
`conf/codedeployagent.yml` for the full template.

| Option | Default | What to test |
|--------|---------|--------------|
| `verbose` | `false` | Set `true` — DEBUG-level logs appear (connection details, sleep timers) |
| `wait_between_runs` | `30` | Set `3`–`5` — polling interval in seconds, visible in logs as `poll_interval_ms` |
| `log_dir` | `/var/log/aws/codedeploy-agent` | Point to `/tmp/...` — logs appear there |
| `pid_dir` | `/opt/codedeploy-agent/state/.pid` | Point to `/tmp/...` — PID file created there |
| `root_dir` | `/opt/codedeploy-agent/deployment-root` | Point to `/tmp/...` — deployment dirs created there |
| `use_fips_mode` | `false` | Set `true` with a US region — endpoint becomes `codedeploy-commands-fips.{region}.amazonaws.com` |
| `deploy_control_endpoint` | _(auto)_ | Set to `https://localhost:9999` — agent connects there instead |
| `http_read_timeout` | `80` | Set lower (e.g. `5`) — faster timeout on connection errors |
| `max_revisions` | `5` | Controls deployment revision cleanup (testable once deployments work) |
| `enable_auth_policy` | `false` | Set `true` — endpoint becomes `codedeploy-commands-secure.{region}.amazonaws.com` |
| `disable_imds_v1` | `false` | Only affects IMDS region resolution (no effect when `AWS_REGION` is set) |

### Region resolution

The agent resolves its AWS region using this chain:

1. On-premises config file
   (`/etc/codedeploy-agent/conf/codedeploy.onpremises.yml`) — `region` key
2. `AWS_REGION` environment variable
3. IMDS identity document (EC2 only)

For local dev without IMDS, set `AWS_REGION`:

```bash
AWS_REGION=us-east-1 $AGENT start &
```

### Expected behavior locally

Without real IAM credentials (InstanceProfile mode), the agent will:

1. Start master + worker processes
2. Load config and create directories
3. Initialize logging
4. Resolve region from `AWS_REGION`
5. Connect to `codedeploy-commands.{region}.amazonaws.com`
6. Get `"The security token included in the request is invalid"` errors
   (expected — placeholder credentials)
7. Back off with exponential delay (capped at ~89s)
8. Shut down cleanly on `stop` or Ctrl-C

## End-to-end testing with real credentials

The `scripts/` directory has a matrix of E2E scripts that each spin up a real
deployment against AWS. They share a common subcommand interface:

```
setup    Create AWS resources (IAM, EC2, S3, CodeDeploy app + group)
run      Start the agent (foreground for on-prem, background-via-SSM for EC2)
deploy   Trigger a deployment
status   Poll until terminal state, print PASS / FAIL banner
logs     Tail the agent log
teardown Delete every resource the script created
all      Full cycle: setup → run (background) → deploy → status → logs → teardown
```

Resources are uniformly named `codedeploy-agent-<test-type>-…` so two tests
never collide. State is saved to `/tmp/codedeploy-agent-<test-type>-state.json`
so `teardown` works across terminal sessions.

### The test matrix

| Script | Where the agent runs | Credential mode |
|---|---|---|
| `e2e-onprem-iam-user.sh` | This host | On-prem registration, inline access keys (`iam_user_arn`) |
| `e2e-onprem-iam-session.sh` | This host | On-prem registration, INI credentials file (`iam_session_arn`) |
| `e2e-ec2-imds.sh` | Fresh EC2 instance | IMDS (instance profile only) |
| `e2e-ec2-iam-user.sh` | Fresh EC2 instance | On-prem registration, inline keys — IMDS bypassed |
| `e2e-ec2-iam-session.sh` | Fresh EC2 instance | On-prem registration, INI file — IMDS bypassed |
| `e2e-ec2-concurrent.sh` | Fresh EC2 instance | IMDS, runs N parallel deployments (default 3) against the same instance |
| `e2e-windows.sh` | Fresh Windows Server EC2 | IMDS |

### Shortest path to a green E2E run

```bash
./scripts/e2e-onprem-iam-user.sh all
```

That registers this host as on-prem, fires a deployment, asserts success, and
tears everything down. No EC2 instance, no `session-manager-plugin`, ~2 minutes.

For an EC2 variant, pick the credential mode you want to exercise and run `all`
against that script — same shape:

```bash
./scripts/e2e-ec2-imds.sh all                       # most common
./scripts/e2e-ec2-iam-session.sh all                # INI credentials file
CONCURRENCY=5 ./scripts/e2e-ec2-concurrent.sh all   # concurrent stress test
```

### Common environment variables

| Variable | Effect |
|---|---|
| `AWS_REGION` | Defaults to `us-east-1`. |
| `AGENT_S3_URI` | `s3://bucket/key` to a pre-built agent binary; skips the local build. |
| `AGENT_S3_PREFIX` | `s3://bucket/prefix`; suffix `/linux/codedeploy-agent` or `/windows/…exe` is appended. |
| `CONCURRENCY` | `e2e-ec2-concurrent.sh` only — number of parallel deployment groups (1..16, default 3). |
| `DEBUG=1` | Enables `set -x` tracing and a call-stack dump on exit (any script). |

### Offline CLI test

`cli-deploy-local.sh` exercises the agent's `deploy-local` subcommand with no
AWS connectivity. It runs through tar / tgz / zip / directory bundles,
lifecycle-hook events, and failure paths. Useful as a local sanity check before
the AWS-backed E2Es.

```bash
./scripts/cli-deploy-local.sh
```

### Auditing and cleaning up leftover resources

`inventory-test-resources.sh` is read-only by default and scans for
`codedeploy-agent-*` resources. Three modes:

```bash
./scripts/inventory-test-resources.sh                # list
./scripts/inventory-test-resources.sh --dry-run      # render delete plan
./scripts/inventory-test-resources.sh --delete --yes # actually delete
```

By default it scans `us-east-1` and `us-west-2`; pass `--regions a,b,c` or
`--all-regions` to widen the scope. IAM is global so it's scanned once.

### Prerequisites for the E2E scripts

- AWS credentials in the environment with admin access to a sandbox account
- AWS CLI v2, `jq`, `zip`
- A built agent binary (`cargo build --release`) **or** `AGENT_S3_URI` /
  `AGENT_S3_PREFIX` pointing at a pre-built one

## Resources

- [Rust Book](https://doc.rust-lang.org/book/)
- [Rust by Example](https://doc.rust-lang.org/rust-by-example/)
- [Cargo Book](https://doc.rust-lang.org/cargo/)
- [Clippy Lints](https://rust-lang.github.io/rust-clippy/master/)
- [Rustfmt Configuration](https://rust-lang.github.io/rustfmt/)
