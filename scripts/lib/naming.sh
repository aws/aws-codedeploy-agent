#!/usr/bin/env bash
# shellcheck disable=SC2034  # variables set here are documented out-params for callers
# ──────────────────────────────────────────────────────────────────────
# scripts/lib/naming.sh — standardized AWS resource names per test.
#
# Source this from another script. The script must already define
# `info()` and `die()`.
#
#   SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
#   source "${SCRIPT_DIR}/lib/naming.sh"
#   naming_init "ec2-iam-session"
#
# After `naming_init`, every standardized name is exported as a
# read-only-by-convention global. All scripts use the same shape so
# values can never collide across the suite (the TEST_TYPE segment
# disambiguates) and so it's trivial to grep AWS for "every test
# resource" via the common prefix `codedeploy-agent-`.
#
# ── Inputs ──
#
#   TEST_TYPE  Required argument to naming_init. Should match the
#              script's filename suffix (e.g. "ec2-iam-session" for
#              scripts/e2e-ec2-iam-session.sh). Use kebab-case.
#
#   AWS_REGION Optional env override; defaults to us-east-1.
#
# ── Outputs (set by naming_init) ──
#
#   TEST_TYPE                 Echoed back from the argument.
#   PREFIX                    codedeploy-agent-<TEST_TYPE>
#                             Used as the prefix for every AWS resource.
#   AWS_ACCOUNT_ID            From sts:GetCallerIdentity.
#   REGION                    Resolved from AWS_REGION env (default us-east-1).
#
#   APP_NAME                  ${PREFIX}-app
#   DG_NAME                   ${PREFIX}-dg
#   DG_BASE                   ${PREFIX}-dg   (alias for scripts that suffix -1, -2…)
#   AGENT_USER                ${PREFIX}-user
#   AGENT_ROLE_NAME           ${PREFIX}-instance-role
#   INSTANCE_PROFILE_NAME     ${PREFIX}-instance-profile
#   SERVICE_ROLE_NAME         ${PREFIX}-service-role
#   AGENT_USER_POLICY_NAME    ${PREFIX}-user-policy
#   AGENT_ROLE_POLICY_NAME    ${PREFIX}-instance-policy
#
#   BUCKET_NAME               ${PREFIX}-${AWS_ACCOUNT_ID}
#   BUNDLE_KEY                revision.zip   (constant; bucket is per-test)
#   AGENT_BINARY_KEY          agent-binary   (constant)
#
#   STATE_FILE                /tmp/${PREFIX}-state.json
#   TAG_KEY                   codedeploy-agent-test
#   TAG_VALUE                 ${TEST_TYPE}
#
# ──────────────────────────────────────────────────────────────────────

naming_init() {
    TEST_TYPE="${1:?TEST_TYPE required (e.g. ec2-iam-session)}"

    if ! [[ "$TEST_TYPE" =~ ^[a-z0-9][a-z0-9-]*[a-z0-9]$ ]]; then
        die "naming_init: TEST_TYPE must be kebab-case ([a-z0-9-], no leading/trailing dash), got: '${TEST_TYPE}'"
    fi

    REGION="${AWS_REGION:-us-east-1}"

    PREFIX="codedeploy-agent-${TEST_TYPE}"

    # IAM name max length is 64; the longest suffix is -instance-profile (17 chars).
    local longest_iam="${PREFIX}-instance-profile"
    if (( ${#longest_iam} > 64 )); then
        die "naming_init: TEST_TYPE '${TEST_TYPE}' makes IAM names too long (${#longest_iam} > 64 chars)"
    fi

    AWS_ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text 2>/dev/null) \
        || die "naming_init: aws sts get-caller-identity failed; no valid credentials?"

    APP_NAME="${PREFIX}-app"
    DG_NAME="${PREFIX}-dg"
    DG_BASE="${PREFIX}-dg"
    AGENT_USER="${PREFIX}-user"
    AGENT_ROLE_NAME="${PREFIX}-instance-role"
    INSTANCE_PROFILE_NAME="${PREFIX}-instance-profile"
    SERVICE_ROLE_NAME="${PREFIX}-service-role"
    AGENT_USER_POLICY_NAME="${PREFIX}-user-policy"
    AGENT_ROLE_POLICY_NAME="${PREFIX}-instance-policy"

    BUCKET_NAME="${PREFIX}-${AWS_ACCOUNT_ID}"
    BUNDLE_KEY="revision.zip"
    AGENT_BINARY_KEY="agent-binary"

    STATE_FILE="/tmp/${PREFIX}-state.json"
    TAG_KEY="codedeploy-agent-test"
    TAG_VALUE="${TEST_TYPE}"

    # S3 bucket name max length is 63.
    if (( ${#BUCKET_NAME} > 63 )); then
        die "naming_init: bucket name too long: '${BUCKET_NAME}' (${#BUCKET_NAME} > 63)"
    fi
}
