#!/usr/bin/env bash
# ============================================================
# cli-deploy-local.sh — E2E test for the deploy-local subcommand
#
# Validates the local deployment CLI without any AWS connectivity.
# Exercises: directory bundles, tar bundles, tgz bundles, zip bundles,
# file installation, lifecycle hooks, env vars, event filtering,
# error handling, and auto-detection of bundle type.
#
# Usage:
#   cargo build --release         # build first (or set AGENT_S3_URI / AGENT_S3_PREFIX)
#   ./scripts/cli-deploy-local.sh
#
# Env: set AGENT_S3_URI=s3://bucket/key or AGENT_S3_PREFIX=s3://bucket/prefix
# to pull a pre-built Linux binary instead of relying on a local build.
#
# No root required (uses /tmp paths). No AWS credentials needed.
# ============================================================
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
AGENT="$REPO_ROOT/target/release/codedeploy-agent"

PASS=0
FAIL=0
WORK_DIR=""

# Minimal info/die so the shared reporting helpers work.
info() { echo "==> $*"; }
die()  { echo "ERROR: $*" >&2; exit 1; }

# Shared deployment-status reporting helpers (only print_banner is used here).
# shellcheck source=lib/reporting.sh
source "${SCRIPT_DIR}/lib/reporting.sh"
# shellcheck source=lib/agent-source.sh
source "${SCRIPT_DIR}/lib/agent-source.sh"

pass() { ((PASS++)); echo "[PASS] $1"; }
fail() { ((FAIL++)); echo "[FAIL] $1"; }

cleanup() {
    rm -rf "$WORK_DIR" /tmp/deploy-local-test-output 2>/dev/null
    if [[ $FAIL -eq 0 ]]; then
        print_banner "PASS" "deploy-local — all tests passed" \
            "Total cases: $((PASS + FAIL))" \
            "Passed: ${PASS}  Failed: ${FAIL}"
    else
        print_banner "FAIL" "deploy-local — some tests failed" \
            "Total cases: $((PASS + FAIL))" \
            "Passed: ${PASS}  Failed: ${FAIL}"
    fi
    exit $FAIL
}
trap cleanup EXIT

# ── Prereqs ──────────────────────────────────────────────────
ensure_local_agent linux "$AGENT"

WORK_DIR=$(mktemp -d /tmp/test-deploy-local-XXXXXX)
OUTPUT_DIR="/tmp/deploy-local-test-output"

# Ensure deployment root is writable
mkdir -p /opt/codedeploy-agent 2>/dev/null || true

# ── Helper: create a test bundle directory ────────────────────
create_test_bundle() {
    local dir="$1"
    mkdir -p "$dir/scripts" "$dir/files"

    cat > "$dir/appspec.yml" <<'EOF'
version: 0.0
os: linux
files:
  - source: files/config.txt
    destination: /tmp/deploy-local-test-output
hooks:
  BeforeInstall:
    - location: scripts/before.sh
      timeout: 10
  AfterInstall:
    - location: scripts/after.sh
      timeout: 10
EOF

    echo "config_value=42" > "$dir/files/config.txt"

    cat > "$dir/scripts/before.sh" <<'EOF'
#!/bin/bash
echo "BEFORE_INSTALL_RAN=true"
echo "ENV_DEPLOYMENT_ID=$DEPLOYMENT_ID"
echo "ENV_LIFECYCLE_EVENT=$LIFECYCLE_EVENT"
echo "ENV_APPLICATION_NAME=$APPLICATION_NAME"
EOF
    chmod +x "$dir/scripts/before.sh"

    cat > "$dir/scripts/after.sh" <<'EOF'
#!/bin/bash
echo "AFTER_INSTALL_RAN=true"
if [[ -f /tmp/deploy-local-test-output/config.txt ]]; then
    echo "FILE_INSTALLED=true"
else
    echo "FILE_INSTALLED=false"
fi
EOF
    chmod +x "$dir/scripts/after.sh"
}

# ── TC-1: Directory bundle — full lifecycle ───────────────────
echo ""
echo "=== TC-1: Directory bundle — full lifecycle ==="
rm -rf "$OUTPUT_DIR"
BUNDLE="$WORK_DIR/bundle-dir"
create_test_bundle "$BUNDLE"

OUTPUT=$("$AGENT" deploy-local -l "$BUNDLE" -t directory 2>&1)
EXIT_CODE=$?

if [[ $EXIT_CODE -eq 0 ]]; then
    pass "TC-1: deploy-local exits 0"
else
    fail "TC-1: deploy-local exit code $EXIT_CODE"
    echo "$OUTPUT"
fi

if [[ -f "$OUTPUT_DIR/config.txt" ]]; then
    pass "TC-1: File installed to destination"
else
    fail "TC-1: File not found at $OUTPUT_DIR/config.txt"
fi

if echo "$OUTPUT" | grep -q "DownloadBundle succeeded"; then
    pass "TC-1: DownloadBundle reported success"
else
    fail "TC-1: DownloadBundle not reported"
fi

if echo "$OUTPUT" | grep -q "Install succeeded"; then
    pass "TC-1: Install reported success"
else
    fail "TC-1: Install not reported"
fi

if echo "$OUTPUT" | grep -q "AfterInstall succeeded"; then
    pass "TC-1: AfterInstall reported success"
else
    fail "TC-1: AfterInstall not reported"
fi

# ── TC-2: Tar bundle ──────────────────────────────────────────
echo ""
echo "=== TC-2: Tar bundle ==="
rm -rf "$OUTPUT_DIR"
TAR_BUNDLE="$WORK_DIR/bundle.tar"
tar cf "$TAR_BUNDLE" -C "$BUNDLE" .

OUTPUT=$("$AGENT" deploy-local -l "$TAR_BUNDLE" -t tar 2>&1)
EXIT_CODE=$?

if [[ $EXIT_CODE -eq 0 ]]; then
    pass "TC-2: tar bundle deploy succeeds"
else
    fail "TC-2: tar bundle failed (exit $EXIT_CODE)"
    echo "$OUTPUT"
fi

if [[ -f "$OUTPUT_DIR/config.txt" ]]; then
    pass "TC-2: File installed from tar bundle"
else
    fail "TC-2: File not installed from tar bundle"
fi

# ── TC-3: Tgz bundle ─────────────────────────────────────────
echo ""
echo "=== TC-3: Tgz bundle ==="
rm -rf "$OUTPUT_DIR"
TGZ_BUNDLE="$WORK_DIR/bundle.tgz"
tar czf "$TGZ_BUNDLE" -C "$BUNDLE" .

OUTPUT=$("$AGENT" deploy-local -l "$TGZ_BUNDLE" -t tgz 2>&1)
EXIT_CODE=$?

if [[ $EXIT_CODE -eq 0 ]]; then
    pass "TC-3: tgz bundle deploy succeeds"
else
    fail "TC-3: tgz bundle failed (exit $EXIT_CODE)"
    echo "$OUTPUT"
fi

# ── TC-4: Zip bundle ─────────────────────────────────────────
echo ""
echo "=== TC-4: Zip bundle ==="
rm -rf "$OUTPUT_DIR"
ZIP_BUNDLE="$WORK_DIR/bundle.zip"
(cd "$BUNDLE" && zip -r "$ZIP_BUNDLE" . >/dev/null 2>&1)

if [[ -f "$ZIP_BUNDLE" ]]; then
    OUTPUT=$("$AGENT" deploy-local -l "$ZIP_BUNDLE" -t zip 2>&1)
    EXIT_CODE=$?

    if [[ $EXIT_CODE -eq 0 ]]; then
        pass "TC-4: zip bundle deploy succeeds"
    else
        fail "TC-4: zip bundle failed (exit $EXIT_CODE)"
        echo "$OUTPUT"
    fi
else
    pass "TC-4: SKIPPED (zip command not available)"
fi

# ── TC-5: Event filtering — only run AfterInstall ─────────────
echo ""
echo "=== TC-5: Event filtering ==="
rm -rf "$OUTPUT_DIR"

OUTPUT=$("$AGENT" deploy-local -l "$BUNDLE" -t directory -e AfterInstall 2>&1)
EXIT_CODE=$?

if [[ $EXIT_CODE -eq 0 ]]; then
    pass "TC-5: deploy with event filter succeeds"
else
    fail "TC-5: deploy with event filter failed (exit $EXIT_CODE)"
fi

if echo "$OUTPUT" | grep -q "BeforeInstall"; then
    fail "TC-5: BeforeInstall ran despite being filtered out"
else
    pass "TC-5: BeforeInstall correctly skipped"
fi

if echo "$OUTPUT" | grep -q "AfterInstall succeeded"; then
    pass "TC-5: AfterInstall ran as specified"
else
    fail "TC-5: AfterInstall did not run"
fi

# ── TC-6: Non-existent bundle — error handling ────────────────
echo ""
echo "=== TC-6: Non-existent bundle ==="

OUTPUT=$("$AGENT" deploy-local -l /tmp/no-such-bundle-xyz.tar 2>&1)
EXIT_CODE=$?

if [[ $EXIT_CODE -ne 0 ]]; then
    pass "TC-6: Non-existent bundle exits non-zero"
else
    fail "TC-6: Non-existent bundle should fail"
fi

if echo "$OUTPUT" | grep -q "not found"; then
    pass "TC-6: Error message mentions 'not found'"
else
    fail "TC-6: Error message unclear: $OUTPUT"
fi

# ── TC-7: Missing appspec — error handling ────────────────────
echo ""
echo "=== TC-7: Missing appspec ==="
BAD_BUNDLE="$WORK_DIR/bad-bundle"
mkdir -p "$BAD_BUNDLE"
echo "no appspec" > "$BAD_BUNDLE/readme.txt"

OUTPUT=$("$AGENT" deploy-local -l "$BAD_BUNDLE" -t directory 2>&1)
EXIT_CODE=$?

if [[ $EXIT_CODE -ne 0 ]]; then
    pass "TC-7: Missing appspec exits non-zero"
else
    fail "TC-7: Missing appspec should fail"
fi

if echo "$OUTPUT" | grep -q "appspec"; then
    pass "TC-7: Error message mentions appspec"
else
    fail "TC-7: Error message unclear: $OUTPUT"
fi

# ── TC-8: Custom appspec filename ─────────────────────────────
echo ""
echo "=== TC-8: Custom appspec filename ==="
rm -rf "$OUTPUT_DIR"
CUSTOM_BUNDLE="$WORK_DIR/custom-appspec"
mkdir -p "$CUSTOM_BUNDLE/scripts" "$CUSTOM_BUNDLE/files"
echo "custom_config=yes" > "$CUSTOM_BUNDLE/files/config.txt"
cat > "$CUSTOM_BUNDLE/scripts/hook.sh" <<'EOF'
#!/bin/bash
echo "CUSTOM_APPSPEC_HOOK_RAN=true"
EOF
chmod +x "$CUSTOM_BUNDLE/scripts/hook.sh"
cat > "$CUSTOM_BUNDLE/my-appspec.yml" <<'EOF'
version: 0.0
os: linux
files:
  - source: files/config.txt
    destination: /tmp/deploy-local-test-output
hooks:
  AfterInstall:
    - location: scripts/hook.sh
      timeout: 10
EOF

OUTPUT=$("$AGENT" deploy-local -l "$CUSTOM_BUNDLE" -t directory --appspec-filename my-appspec.yml -e AfterInstall 2>&1)
EXIT_CODE=$?

if [[ $EXIT_CODE -eq 0 ]]; then
    pass "TC-8: Custom appspec filename works"
else
    fail "TC-8: Custom appspec filename failed (exit $EXIT_CODE)"
    echo "$OUTPUT"
fi

# ── TC-9: Hook script failure — propagates error ──────────────
echo ""
echo "=== TC-9: Failing hook script ==="
FAIL_BUNDLE="$WORK_DIR/fail-bundle"
mkdir -p "$FAIL_BUNDLE/scripts"
cat > "$FAIL_BUNDLE/appspec.yml" <<'EOF'
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/fail.sh
      timeout: 10
EOF
cat > "$FAIL_BUNDLE/scripts/fail.sh" <<'EOF'
#!/bin/bash
echo "about to fail"
exit 1
EOF
chmod +x "$FAIL_BUNDLE/scripts/fail.sh"

OUTPUT=$("$AGENT" deploy-local -l "$FAIL_BUNDLE" -t directory -e AfterInstall 2>&1)
EXIT_CODE=$?

if [[ $EXIT_CODE -ne 0 ]]; then
    pass "TC-9: Failing hook causes deploy-local to exit non-zero"
else
    fail "TC-9: Failing hook should propagate failure"
fi

if echo "$OUTPUT" | grep -q "AfterInstall failed"; then
    pass "TC-9: Failure message identifies the failing hook"
else
    fail "TC-9: Failure message unclear: $OUTPUT"
fi

# ── TC-10: Deployment group name ──────────────────────────────
echo ""
echo "=== TC-10: Custom deployment group ==="
rm -rf "$OUTPUT_DIR"

OUTPUT=$("$AGENT" deploy-local -l "$BUNDLE" -t directory -g my-custom-group -e AfterInstall 2>&1)
EXIT_CODE=$?

if [[ $EXIT_CODE -eq 0 ]]; then
    pass "TC-10: Custom deployment group accepted"
else
    fail "TC-10: Custom deployment group failed (exit $EXIT_CODE)"
fi
