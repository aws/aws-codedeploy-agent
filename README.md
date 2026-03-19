# aws-codedeploy-agent

## Quick Start

After cloning, run once:
```bash
make setup            # Install dev tools and configure git hooks
```

### Available Commands

**Quick Commands:**
```bash
make help           # Show all available commands
make build          # Build the project
make test           # Run all tests
make ci             # Run all CI checks (format, lint, test)
```

**Development:**
```bash
make fmt            # Format code
make fmt-check      # Check formatting without changes
make lint           # Run clippy linter
make lint-fix       # Auto-fix linting issues
make check          # Run cargo check
```

**Testing & Coverage:**
```bash
make test           # Run all tests
make nextest        # Run tests with cargo-nextest (faster)
make mutants        # Run mutation testing with cargo-mutants
make coverage       # Generate HTML coverage report
make coverage-serve # Generate and serve coverage at localhost:8080
make coverage-ci    # Generate LCOV coverage for CI
cargo test -- --nocapture  # Run tests with output
```

**Code Quality:**
```bash
make unused-deps    # Check for unused dependencies
make unused-deps    # Check for unused dependencies
```

**Build Variants:**
```bash
make build          # Debug build
make build-release  # Release build (optimized)
```

**Utilities:**
```bash
make clean          # Clean build artifacts
make install-tools  # Install dev tools (rustfmt, clippy, etc.)
make watch          # Auto-run tests on file changes (requires cargo-watch)
make doc            # Generate and open documentation
make bench          # Run benchmarks
make outdated       # Check for outdated dependencies
```

**Cargo Aliases** (from `.cargo/config.toml`):
```bash
cargo fmt-check     # Check formatting
cargo lint          # Run clippy
cargo test-all      # Run all tests
cargo coverage      # Generate coverage
```

The project aims for 91% code coverage and uses strict linting. Run `make ci` before committing to ensure everything passes.

For more details, see [DEVELOPMENT.md](DEVELOPMENT.md).

## Useful links
## Running the Agent Locally

Build and locate the binary:

```bash
cargo build --release
AGENT=target/release/aws-codedeploy-agent
```
The agent auto-creates its PID, state, and log directories on startup. Default paths live under `/opt/codedeploy-agent/` (owned by root in production). For local dev, make them writable once:

```bash
sudo mkdir -p /opt/codedeploy-agent
sudo chown -R $USER /opt/codedeploy-agent
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
AGENT=target/release/aws-codedeploy-agent

# Start with a region (required — no IMDS locally)
AWS_REGION=us-east-1 $AGENT start &
sleep 3
$AGENT status                          # "running", exit 0
ps aux | grep aws-codedeploy | grep -v grep # master (start) + worker

# Check logs
cat /opt/codedeploy-agent/logs/codedeploy-agent*

# Stop and verify
$AGENT stop
$AGENT status                          # "not running", exit 3
```

### Using a config file

Pass `--config-file` to override defaults. A sample config lives at `codedeployagent.example.yml`.

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

For quick debugging, run the worker subprocess directly (no master, no PID file):

```bash
# Runs in foreground, Ctrl-C to stop
AWS_REGION=us-east-1 $AGENT --config-file /tmp/codedeploy-dev.yml worker
```

### Testable configuration options

All options can be set in the YAML config file. See `codedeployagent.example.yml` for the full template.

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

### Reproduction steps for each test

All tests below assume you've built and set up the binary:

```bash
cargo build --release
AGENT=target/release/aws-codedeploy-agent
sudo mkdir -p /opt/codedeploy-agent && sudo chown -R $USER /opt/codedeploy-agent
```

**1. Verbose logging + fast polling**

```bash
cat > /tmp/codedeploy-dev.yml << 'EOF'
verbose: true
wait_between_runs: 5
log_dir: /opt/codedeploy-agent/logs
EOF

rm -f /opt/codedeploy-agent/logs/*
AWS_REGION=us-east-1 timeout 12 $AGENT --config-file /tmp/codedeploy-dev.yml worker
cat /opt/codedeploy-agent/logs/codedeploy-agent*
# Expect: DEBUG lines, poll_interval_ms=5000, "Resolved region: us-east-1",
#         endpoint codedeploy-commands.us-east-1.amazonaws.com,
#         "The security token included in the request is invalid" (expected)
```

**2. Full daemon lifecycle (start / status / stop)**

```bash
rm -f /opt/codedeploy-agent/state/.pid/master.pid /opt/codedeploy-agent/logs/*

AWS_REGION=us-east-1 $AGENT --config-file /tmp/codedeploy-dev.yml start &
sleep 3
$AGENT status                          # "running", exit 0
ps aux | grep aws-codedeploy | grep -v grep # master (start) + worker
cat /opt/codedeploy-agent/logs/codedeploy-agent*

$AGENT stop
$AGENT status                          # "not running", exit 3
# Logs should end with "Polling loop stopped" and "Worker ... shutting down"
```

**3. Custom directories**

```bash
cat > /tmp/codedeploy-dirs.yml << 'EOF'
verbose: true
wait_between_runs: 3
log_dir: /tmp/cd-test-logs
pid_dir: /tmp/cd-test-pid
root_dir: /tmp/cd-test-root
EOF

rm -rf /tmp/cd-test-logs /tmp/cd-test-pid /tmp/cd-test-root
AWS_REGION=us-east-1 timeout 8 $AGENT --config-file /tmp/codedeploy-dirs.yml worker
ls /tmp/cd-test-logs/   # codedeploy-agent.YYYY-MM-DD
ls /tmp/cd-test-pid/    # empty (worker mode doesn't write PID)
ls /tmp/cd-test-root/   # ongoing-deployment/
```

**4. FIPS endpoint**

```bash
cat > /tmp/codedeploy-fips.yml << 'EOF'
verbose: true
wait_between_runs: 3
use_fips_mode: true
log_dir: /tmp/cd-test-logs
pid_dir: /tmp/cd-test-pid
root_dir: /tmp/cd-test-root
EOF

rm -f /tmp/cd-test-logs/*
AWS_REGION=us-east-1 timeout 8 $AGENT --config-file /tmp/codedeploy-fips.yml worker
grep "codedeploy-commands" /tmp/cd-test-logs/codedeploy-agent*
# Expect: codedeploy-commands-fips.us-east-1.amazonaws.com
```

**5. Custom endpoint**

```bash
cat > /tmp/codedeploy-endpoint.yml << 'EOF'
verbose: true
wait_between_runs: 3
deploy_control_endpoint: "https://localhost:9999"
log_dir: /tmp/cd-test-logs
pid_dir: /tmp/cd-test-pid
root_dir: /tmp/cd-test-root
EOF

rm -f /tmp/cd-test-logs/*
AWS_REGION=us-east-1 timeout 8 $AGENT --config-file /tmp/codedeploy-endpoint.yml worker
grep "connecting" /tmp/cd-test-logs/codedeploy-agent*
# Expect: connecting to 127.0.0.1:9999
```

**6. Region from AWS_REGION (different regions)**

```bash
rm -f /tmp/cd-test-logs/*
AWS_REGION=us-west-2 timeout 8 $AGENT --config-file /tmp/codedeploy-dirs.yml worker
grep "Resolved region" /tmp/cd-test-logs/codedeploy-agent*
# Expect: Resolved region: us-west-2
grep "codedeploy-commands" /tmp/cd-test-logs/codedeploy-agent*
# Expect: codedeploy-commands.us-west-2.amazonaws.com
```

**7. Exponential backoff**

```bash
rm -f /tmp/cd-test-logs/*
AWS_REGION=us-east-1 timeout 120 $AGENT --config-file /tmp/codedeploy-dirs.yml worker
grep "Sleeping" /tmp/cd-test-logs/codedeploy-agent*
# Expect: sleep_secs escalating: 9, 12, 16, 20, 26, 33, 43, 55, 70, 89, 89 (capped)
```

**8. enable_auth_policy endpoint**

```bash
cat > /tmp/codedeploy-auth.yml << 'EOF'
verbose: true
wait_between_runs: 3
enable_auth_policy: true
log_dir: /tmp/cd-test-logs
pid_dir: /tmp/cd-test-pid
root_dir: /tmp/cd-test-root
EOF

rm -f /tmp/cd-test-logs/*
AWS_REGION=us-east-1 timeout 8 $AGENT --config-file /tmp/codedeploy-auth.yml worker
grep "codedeploy-commands" /tmp/cd-test-logs/codedeploy-agent*
# Expect: codedeploy-commands-secure.us-east-1.amazonaws.com
```

### Region resolution

The agent resolves its AWS region using this chain:

1. On-premises config file (`/etc/codedeploy-agent/conf/codedeploy.onpremises.yml`) — `region` key
2. `AWS_REGION` environment variable
3. IMDS identity document (EC2 only)

For local dev without IMDS, set `AWS_REGION`:

```bash
AWS_REGION=us-east-1 $AGENT start &
```

### Expected behavior locally

Without real IAM credentials (InstanceProfile mode), the agent will:

1. Start master + worker processes ✓
2. Load config and create directories ✓
3. Initialize logging ✓
4. Resolve region from `AWS_REGION` ✓
5. Connect to `codedeploy-commands.{region}.amazonaws.com` ✓
6. Get `"The security token included in the request is invalid"` errors (expected — placeholder credentials) ✓
7. Back off with exponential delay (capped at ~89s) ✓
8. Shut down cleanly on `stop` or Ctrl-C ✓

### Current limitations

- **No real credentials locally**: The agent uses placeholder credentials for InstanceProfile mode. Requests to CodeDeploy will fail with auth errors. Real IMDS credential refresh is 
- **No local deployment execution**: The polling loop runs but cannot process deployments without valid credentials and a CodeDeploy service connection.
- **No `codedeploy-local` equivalent yet**: The Ruby agent has a `codedeploy-local` CLI for testing deployments without AWS. This is not yet implemented in the Rust agent.

### End-to-end testing with real credentials

An automated script at `scripts/e2e-test.sh` sets up all AWS resources needed to run a real deployment through the agent.

#### Prerequisites

- AWS credentials in the environment with admin access (or the permissions listed in the script header). For example:
  ```bash
  export AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... AWS_SESSION_TOKEN=...
  ```
- AWS CLI v2 installed
- `jq` installed
- Agent binary built (`cargo build --release`)

#### Interactive workflow (two terminals)

```bash
# Terminal 1: Setup and run the agent
./scripts/e2e-test.sh setup    # Creates IAM user, registers on-premises instance,
                                # creates CodeDeploy app + deployment group,
                                # uploads sample revision to S3 (~30s)
./scripts/e2e-test.sh run      # Starts agent in foreground (Ctrl-C to stop)

# Terminal 2: Trigger and monitor a deployment
./scripts/e2e-test.sh deploy   # Creates a deployment
./scripts/e2e-test.sh status   # Check deployment progress
tail -f /tmp/acdc-e2e-test/logs/codedeploy-agent*  # Watch agent logs

# When done (either terminal)
./scripts/e2e-test.sh teardown # Deletes all AWS resources and local state
```

#### Fully automated (single command)

```bash
./scripts/e2e-test.sh all      # setup → run → deploy → wait → status → teardown
```

#### What the script creates

| Resource | Name | Purpose |
|----------|------|---------|
| IAM user | `acdc-e2e-test-agent-user` | On-premises agent credentials (`codedeploy-commands:*`, S3 read) |
| IAM role | `acdc-e2e-test-codedeploy-role` | CodeDeploy service role |
| On-premises instance | `acdc-e2e-test-<hostname>` | Registered with CodeDeploy, tagged `e2e-test=true` |
| CodeDeploy application | `acdc-e2e-test-app` | Test application |
| Deployment group | `acdc-e2e-test-dg` | Targets the on-premises instance |
| S3 bucket | `acdc-e2e-test-revisions-<account-id>` | Holds the sample revision (appspec + hook scripts) |

All resources are cleaned up by `teardown`. State is saved to `/tmp/acdc-e2e-test-state.json` so `teardown` works even across terminal sessions.

