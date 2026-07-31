#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# End-to-end test for the Rust CodeDeploy agent.
#
# Registers this host as an on-premises instance, creates a CodeDeploy
# application + deployment group, uploads a sample revision to S3, and
# triggers a deployment that the locally-running agent processes.
#
# Usage:
#   ./scripts/e2e-onprem-iam-user.sh setup     # Create all AWS resources
#   ./scripts/e2e-onprem-iam-user.sh deploy    # Trigger a deployment
#   ./scripts/e2e-onprem-iam-user.sh run       # Start the agent (foreground)
#   ./scripts/e2e-onprem-iam-user.sh status    # Check deployment status
#   ./scripts/e2e-onprem-iam-user.sh teardown  # Delete all AWS resources
#   ./scripts/e2e-onprem-iam-user.sh all       # setup + run (background) + deploy + status + teardown
#
# Prerequisites:
#   - AWS credentials in the environment (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY,
#     AWS_SESSION_TOKEN, or a profile). Needs permissions listed below.
#   - AWS CLI v2 installed
#   - Agent binary built: cargo build --release (or set AGENT_S3_URI / AGENT_S3_PREFIX)
#   - jq installed
#
# Optional: skip `cargo build --release` and pull a pre-built binary from S3.
#   AGENT_S3_URI    full s3://bucket/key (overrides AGENT_S3_PREFIX)
#   AGENT_S3_PREFIX s3://bucket/prefix; suffix /linux/codedeploy-agent is appended
#
# Required IAM permissions (or use an admin role):
#   - iam:CreateUser, iam:CreateAccessKey, iam:PutUserPolicy, iam:DeleteUser,
#     iam:DeleteUserPolicy, iam:DeleteAccessKey, iam:ListAccessKeys
#   - codedeploy:CreateApplication, codedeploy:CreateDeploymentGroup,
#     codedeploy:CreateDeployment, codedeploy:GetDeployment,
#     codedeploy:RegisterOnPremisesInstance, codedeploy:DeregisterOnPremisesInstance,
#     codedeploy:RemoveTagsFromOnPremisesInstances, codedeploy:AddTagsToOnPremisesInstances,
#     codedeploy:DeleteApplication, codedeploy:DeleteDeploymentGroup
#   - iam:CreateRole, iam:PutRolePolicy, iam:DeleteRole, iam:DeleteRolePolicy,
#     iam:PassRole
#   - s3:CreateBucket, s3:PutObject, s3:DeleteObject, s3:DeleteBucket,
#     s3:GetObject, s3:ListBucket
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

# ── Configuration ────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
AGENT="${REPO_ROOT}/target/release/codedeploy-agent"

# ── Helpers ──────────────────────────────────────────────────────────
die()  { echo "ERROR: $*" >&2; exit 1; }
info() { echo "==> $*"; }

# Shared libraries (reporting, agent-source, naming).
# shellcheck source=lib/reporting.sh
source "${SCRIPT_DIR}/lib/reporting.sh"
# shellcheck source=lib/agent-source.sh
source "${SCRIPT_DIR}/lib/agent-source.sh"
# shellcheck source=lib/naming.sh
source "${SCRIPT_DIR}/lib/naming.sh"
# shellcheck source=lib/diagnostics.sh
source "${SCRIPT_DIR}/lib/diagnostics.sh"
diagnostics_install

# Standardize all AWS resource names from a single TEST_TYPE input.
naming_init "onprem-iam-user"

# Per-script extras.
INSTANCE_NAME="${PREFIX}-$(hostname -s)"
E2E_DIR="/tmp/${PREFIX}"
CONFIG_FILE="${E2E_DIR}/codedeployagent.yml"
ONPREM_CONFIG="/tmp/${PREFIX}-onpremises.yml"
LOG_DIR="${E2E_DIR}/logs"
PID_DIR="${E2E_DIR}/pid"
ROOT_DIR="${E2E_DIR}/deployment-root"

check_prereqs() {
    ensure_local_agent linux "$AGENT"
    command -v aws >/dev/null || die "AWS CLI not found"
    command -v jq >/dev/null || die "jq not found"
    aws sts get-caller-identity >/dev/null 2>&1 || die "No valid AWS credentials in environment"
}

save_state() { echo "$1" | jq -S '.' > "$STATE_FILE"; }
load_state() { cat "$STATE_FILE" 2>/dev/null || echo '{}'; }

# ── Setup ────────────────────────────────────────────────────────────
cmd_setup() {
    check_prereqs
    info "Setting up e2e test resources in ${REGION}..."

    mkdir -p "$E2E_DIR" "$LOG_DIR" "$PID_DIR" "$ROOT_DIR"

    # 1. Create IAM user for the agent with on-premises credentials.
    info "Creating IAM user: ${AGENT_USER}"
    if aws iam get-user --user-name "$AGENT_USER" --region "$REGION" >/dev/null 2>&1; then
        info "  IAM user already exists, reusing"
    else
        aws iam create-user --user-name "$AGENT_USER" --region "$REGION"
    fi

    aws iam put-user-policy \
        --user-name "$AGENT_USER" \
        --policy-name "${AGENT_USER_POLICY_NAME}" \
        --policy-document '{
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Effect": "Allow",
                    "Action": [
                        "codedeploy-commands:*",
                        "s3:GetObject",
                        "s3:GetObjectVersion",
                        "s3:ListBucket"
                    ],
                    "Resource": "*"
                }
            ]
        }' \
        --region "$REGION"

    # Delete any existing access keys before creating a fresh one.
    EXISTING_KEYS=$(aws iam list-access-keys --user-name "$AGENT_USER" --region "$REGION" --query 'AccessKeyMetadata[].AccessKeyId' --output text)
    for key in $EXISTING_KEYS; do
        info "  Deleting stale access key: ${key}"
        aws iam delete-access-key --user-name "$AGENT_USER" --access-key-id "$key" --region "$REGION"
    done

    KEYS=$(aws iam create-access-key --user-name "$AGENT_USER" --region "$REGION" --output json)
    ACCESS_KEY=$(echo "$KEYS" | jq -r '.AccessKey.AccessKeyId')
    SECRET_KEY=$(echo "$KEYS" | jq -r '.AccessKey.SecretAccessKey')
    info "Agent access key: ${ACCESS_KEY}"

    # 2. Create CodeDeploy service role.
    info "Creating CodeDeploy service role: ${SERVICE_ROLE_NAME}"
    if aws iam get-role --role-name "$SERVICE_ROLE_NAME" --region "$REGION" >/dev/null 2>&1; then
        info "  Service role already exists, reusing"
    else
        aws iam create-role \
            --role-name "$SERVICE_ROLE_NAME" \
            --assume-role-policy-document '{
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "codedeploy.amazonaws.com"},
                    "Action": "sts:AssumeRole"
                }]
            }' \
            --region "$REGION"
    fi

    aws iam attach-role-policy \
        --role-name "$SERVICE_ROLE_NAME" \
        --policy-arn "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole"

    SERVICE_ROLE_ARN=$(aws iam get-role --role-name "$SERVICE_ROLE_NAME" --query 'Role.Arn' --output text --region "$REGION")

    # 3. Register on-premises instance.
    info "Registering on-premises instance: ${INSTANCE_NAME}"
    IAM_USER_ARN=$(aws iam get-user --user-name "$AGENT_USER" --query 'User.Arn' --output text --region "$REGION")

    aws deploy register-on-premises-instance \
        --instance-name "$INSTANCE_NAME" \
        --iam-user-arn "$IAM_USER_ARN" \
        --region "$REGION" 2>/dev/null || true

    aws deploy add-tags-to-on-premises-instances \
        --instance-names "$INSTANCE_NAME" \
        --tags "Key=${TAG_KEY},Value=${TAG_VALUE}" \
        --region "$REGION" 2>/dev/null || true

    # 4. Create CodeDeploy application + deployment group.
    info "Creating application: ${APP_NAME}"
    if aws deploy get-application --application-name "$APP_NAME" --region "$REGION" >/dev/null 2>&1; then
        info "  Application already exists, reusing"
    else
        aws deploy create-application \
            --application-name "$APP_NAME" \
            --compute-platform Server \
            --region "$REGION"
    fi

    info "Creating deployment group: ${DG_NAME}"
    if aws deploy get-deployment-group --application-name "$APP_NAME" --deployment-group-name "$DG_NAME" --region "$REGION" >/dev/null 2>&1; then
        info "  Deployment group already exists, updating"
        aws deploy update-deployment-group \
            --application-name "$APP_NAME" \
            --current-deployment-group-name "$DG_NAME" \
            --on-premises-instance-tag-filters "Key=${TAG_KEY},Value=${TAG_VALUE},Type=KEY_AND_VALUE" \
            --service-role-arn "$SERVICE_ROLE_ARN" \
            --region "$REGION"
    else
        info "Waiting for IAM role to propagate..."
        sleep 10
        aws deploy create-deployment-group \
            --application-name "$APP_NAME" \
            --deployment-group-name "$DG_NAME" \
            --on-premises-instance-tag-filters "Key=${TAG_KEY},Value=${TAG_VALUE},Type=KEY_AND_VALUE" \
            --service-role-arn "$SERVICE_ROLE_ARN" \
            --region "$REGION"
    fi

    # Flat wait for IAM role propagation; deploy command retries on IAM_ROLE_PERMISSIONS.
    info "Waiting for IAM role to propagate..."
    sleep 15

    # 5. Create S3 bucket + sample revision.
    info "Creating S3 bucket: ${BUCKET_NAME}"
    if aws s3api head-bucket --bucket "$BUCKET_NAME" --region "$REGION" 2>/dev/null; then
        info "  Bucket already exists, reusing"
    elif [[ "$REGION" == "us-east-1" ]]; then
        aws s3api create-bucket --bucket "$BUCKET_NAME" --region "$REGION"
    else
        aws s3api create-bucket --bucket "$BUCKET_NAME" --region "$REGION" \
            --create-bucket-configuration "LocationConstraint=${REGION}"
    fi

    REVISION_DIR=$(mktemp -d)
    cat > "${REVISION_DIR}/appspec.yml" << 'APPSPEC'
version: 0.0
os: linux
hooks:
  BeforeInstall:
    - location: scripts/before_install.sh
      timeout: 30
  AfterInstall:
    - location: scripts/after_install.sh
      timeout: 30
APPSPEC

    mkdir -p "${REVISION_DIR}/scripts"
    cat > "${REVISION_DIR}/scripts/before_install.sh" << 'SCRIPT'
#!/bin/bash
echo "BeforeInstall hook running at $(date)"
echo "DEPLOYMENT_ID=$DEPLOYMENT_ID"
echo "LIFECYCLE_EVENT=$LIFECYCLE_EVENT"
SCRIPT
    chmod +x "${REVISION_DIR}/scripts/before_install.sh"

    cat > "${REVISION_DIR}/scripts/after_install.sh" << 'SCRIPT'
#!/bin/bash
echo "AfterInstall hook running at $(date)"
echo "DEPLOYMENT_ID=$DEPLOYMENT_ID"
echo "LIFECYCLE_EVENT=$LIFECYCLE_EVENT"
echo "Deployment successful!"
SCRIPT
    chmod +x "${REVISION_DIR}/scripts/after_install.sh"

    REVISION_ZIP="/tmp/${PREFIX}-revision.zip"
    (cd "$REVISION_DIR" && zip -r "$REVISION_ZIP" .)
    aws s3 cp "$REVISION_ZIP" "s3://${BUCKET_NAME}/revision.zip" --region "$REGION"
    rm -rf "$REVISION_DIR" "$REVISION_ZIP"

    # 6. Write agent config files.
    info "Writing agent config to ${CONFIG_FILE}"
    cat > "$CONFIG_FILE" << EOF
verbose: true
wait_between_runs: 5
log_dir: ${LOG_DIR}
pid_dir: ${PID_DIR}
root_dir: ${ROOT_DIR}
on_premises_config_file: ${ONPREM_CONFIG}
EOF

    info "Writing on-premises config to ${ONPREM_CONFIG}"
    cat > "$ONPREM_CONFIG" << EOF
region: ${REGION}
aws_access_key_id: ${ACCESS_KEY}
aws_secret_access_key: ${SECRET_KEY}
iam_user_arn: ${IAM_USER_ARN}
EOF
    chmod 600 "$ONPREM_CONFIG"

    # 7. Save state for other commands.
    save_state "$(cat <<EOF
{
    "region": "${REGION}",
    "app_name": "${APP_NAME}",
    "dg_name": "${DG_NAME}",
    "instance_name": "${INSTANCE_NAME}",
    "agent_user": "${AGENT_USER}",
    "access_key_id": "${ACCESS_KEY}",
    "service_role_name": "${SERVICE_ROLE_NAME}",
    "service_role_arn": "${SERVICE_ROLE_ARN}",
    "bucket_name": "${BUCKET_NAME}",
    "config_file": "${CONFIG_FILE}",
    "iam_user_arn": "${IAM_USER_ARN}"
}
EOF
)"

    info "Setup complete! State saved to ${STATE_FILE}"
    info ""
    info "Next steps:"
    info "  1. ./scripts/e2e-onprem-iam-user.sh run       # Start the agent"
    info "  2. ./scripts/e2e-onprem-iam-user.sh deploy     # Trigger a deployment (in another terminal)"
    info "  3. ./scripts/e2e-onprem-iam-user.sh status     # Check deployment status"
    info "  4. tail -f ${LOG_DIR}/codedeploy-agent*  # Watch logs"
}

# ── Run agent ────────────────────────────────────────────────────────
cmd_run() {
    check_prereqs
    STATE=$(load_state)
    CONFIG_FILE=$(echo "$STATE" | jq -r '.config_file')
    [[ "$CONFIG_FILE" != "null" ]] || die "No state found — run 'setup' first"

    info "Starting agent in foreground (Ctrl-C to stop)..."
    info "Logs: tail -f ${LOG_DIR}/codedeploy-agent*"
    CODEDEPLOY_DEVELOPER_MODE=true "$AGENT" --config-file "$CONFIG_FILE" worker
}

# ── Deploy ───────────────────────────────────────────────────────────
cmd_deploy() {
    check_prereqs
    STATE=$(load_state)
    APP=$(echo "$STATE" | jq -r '.app_name')
    DG=$(echo "$STATE" | jq -r '.dg_name')
    BUCKET=$(echo "$STATE" | jq -r '.bucket_name')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')
    [[ "$APP" != "null" ]] || die "No state found — run 'setup' first"

    info "Creating deployment for ${APP} / ${DG}..."

    # Retry on IAM_ROLE_PERMISSIONS (sync or async) for up to ~2 minutes.
    for attempt in $(seq 1 12); do
        DEPLOY_OUTPUT=$(AWS_PAGER='' aws deploy create-deployment \
            --application-name "$APP" \
            --deployment-group-name "$DG" \
            --revision "revisionType=S3,s3Location={bucket=${BUCKET},key=revision.zip,bundleType=zip}" \
            --region "$REGION_STATE" 2>&1) || true

        # Synchronous IAM rejection.
        if echo "$DEPLOY_OUTPUT" | grep -q "IAM_ROLE_PERMISSIONS"; then
            info "  Role not yet assumable (sync), retrying... (${attempt}/12)"
            sleep 10
            continue
        fi

        DEPLOYMENT_ID=$(echo "$DEPLOY_OUTPUT" | jq -r '.deploymentId // empty' 2>/dev/null)
        [[ -n "$DEPLOYMENT_ID" ]] || die "create-deployment failed: ${DEPLOY_OUTPUT}"

        # Async IAM rejection — accepted but fails within seconds.
        sleep 3
        DEP_ERROR=$(aws deploy get-deployment --deployment-id "$DEPLOYMENT_ID" \
            --query 'deploymentInfo.errorInformation.code' --output text \
            --region "$REGION_STATE" 2>/dev/null) || true
        if [[ "$DEP_ERROR" == "IAM_ROLE_PERMISSIONS" ]]; then
            info "  Role not yet assumable (async), retrying... (${attempt}/12)"
            sleep 10
            continue
        fi

        break
    done

    [[ -n "$DEPLOYMENT_ID" ]] || die "Gave up waiting for IAM role propagation after 12 attempts"

    info "Deployment created: ${DEPLOYMENT_ID}"
    info "Watch progress: ./scripts/e2e-onprem-iam-user.sh status"
    info "Watch logs:     tail -f ${LOG_DIR}/codedeploy-agent*"

    # Update state with deployment ID.
    echo "$STATE" | jq --arg id "$DEPLOYMENT_ID" '.last_deployment_id = $id' > "$STATE_FILE"
}

# ── Status ───────────────────────────────────────────────────────────
cmd_status() {
    check_prereqs
    STATE=$(load_state)
    DEPLOYMENT_ID=$(echo "$STATE" | jq -r '.last_deployment_id // empty')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')
    [[ -n "$DEPLOYMENT_ID" ]] || die "No deployment found — run 'deploy' first"

    info "Polling deployment ${DEPLOYMENT_ID}..."
    if ! wait_for_deployments "$REGION_STATE" 600 "$DEPLOYMENT_ID"; then
        print_banner "TIMEOUT" "On-prem E2E — deployment did not reach terminal state in 10 min"
        die "Timeout"
    fi

    info "Final status:"
    print_deployment_table "$REGION_STATE" "$DEPLOYMENT_ID"

    if (( DEPLOY_SUCCEEDED == DEPLOY_TOTAL )); then
        print_banner "PASS" "On-prem E2E — deployment Succeeded" \
            "Deployment: ${DEPLOYMENT_ID}" \
            "Elapsed: ${DEPLOY_ELAPSED}s"
        print_deployment_outcome SUCCEEDED
    else
        print_banner "FAIL" "On-prem E2E — deployment did not succeed" \
            "Deployment: ${DEPLOYMENT_ID}" \
            "Succeeded: ${DEPLOY_SUCCEEDED}  Failed: ${DEPLOY_FAILED}  Stopped: ${DEPLOY_STOPPED}" \
            "Elapsed: ${DEPLOY_ELAPSED}s"
        print_deployment_outcome FAILED
        return 1
    fi
}

# ── Teardown ─────────────────────────────────────────────────────────
cmd_teardown() {
    info "Tearing down e2e test resources..."
    STATE=$(load_state)
    REGION_STATE=$(echo "$STATE" | jq -r '.region // "us-east-1"')
    APP=$(echo "$STATE" | jq -r '.app_name // empty')
    INSTANCE=$(echo "$STATE" | jq -r '.instance_name // empty')
    USER=$(echo "$STATE" | jq -r '.agent_user // empty')
    ACCESS_KEY_ID=$(echo "$STATE" | jq -r '.access_key_id // empty')
    ROLE=$(echo "$STATE" | jq -r '.service_role_name // empty')
    BUCKET=$(echo "$STATE" | jq -r '.bucket_name // empty')

    # Stop agent if running.
    pkill -f "codedeploy-agent" 2>/dev/null || true

    # Deregister on-premises instance.
    if [[ -n "$INSTANCE" ]]; then
        info "Deregistering instance: ${INSTANCE}"
        aws deploy remove-tags-from-on-premises-instances \
            --instance-names "$INSTANCE" \
            --tags "Key=${TAG_KEY},Value=${TAG_VALUE}" \
            --region "$REGION_STATE" 2>/dev/null || true
        aws deploy deregister-on-premises-instance \
            --instance-name "$INSTANCE" \
            --region "$REGION_STATE" 2>/dev/null || true
    fi

    # Delete CodeDeploy application (cascades to deployment groups).
    if [[ -n "$APP" ]]; then
        info "Deleting application: ${APP}"
        aws deploy delete-application \
            --application-name "$APP" \
            --region "$REGION_STATE" 2>/dev/null || true
    fi

    # Delete IAM user.
    if [[ -n "$USER" ]]; then
        info "Deleting IAM user: ${USER}"
        if [[ -n "$ACCESS_KEY_ID" ]]; then
            aws iam delete-access-key --user-name "$USER" --access-key-id "$ACCESS_KEY_ID" --region "$REGION_STATE" 2>/dev/null || true
        fi
        aws iam delete-user-policy --user-name "$USER" --policy-name "${AGENT_USER_POLICY_NAME}" --region "$REGION_STATE" 2>/dev/null || true
        aws iam delete-user --user-name "$USER" --region "$REGION_STATE" 2>/dev/null || true
    fi

    # Delete service role.
    if [[ -n "$ROLE" ]]; then
        info "Deleting service role: ${ROLE}"
        aws iam detach-role-policy --role-name "$ROLE" --policy-arn "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole" 2>/dev/null || true
        aws iam delete-role-policy --role-name "$ROLE" --policy-name "${AGENT_ROLE_POLICY_NAME}" 2>/dev/null || true
        aws iam delete-role --role-name "$ROLE" 2>/dev/null || true
    fi

    # Delete S3 bucket.
    if [[ -n "$BUCKET" ]]; then
        info "Deleting S3 bucket: ${BUCKET}"
        aws s3 rm "s3://${BUCKET}" --recursive --region "$REGION_STATE" 2>/dev/null || true
        aws s3api delete-bucket --bucket "$BUCKET" --region "$REGION_STATE" 2>/dev/null || true
    fi

    # Clean up local files.
    rm -rf "$E2E_DIR" "$ONPREM_CONFIG" "$STATE_FILE"

    info "Teardown complete."
}

# ── All (full cycle) ─────────────────────────────────────────────────
cmd_all() {
    cmd_setup

    info "Starting agent in background..."
    "$AGENT" --config-file "$(load_state | jq -r '.config_file')" worker &
    AGENT_PID=$!
    sleep 5

    cmd_deploy

    cmd_status || true

    info "Stopping agent..."
    kill "$AGENT_PID" 2>/dev/null || true
    wait "$AGENT_PID" 2>/dev/null || true

    info "Agent logs (tail):"
    tail -n 50 "${LOG_DIR}"/codedeploy-agent* 2>/dev/null || true

    replay_deployment_outcome
    cmd_teardown
}

# ── Main ─────────────────────────────────────────────────────────────
case "${1:-help}" in
    setup)    cmd_setup ;;
    run)      cmd_run ;;
    deploy)   cmd_deploy ;;
    status)   cmd_status ;;
    teardown) cmd_teardown ;;
    all)      cmd_all ;;
    help|*)
        echo "Usage: $0 {setup|run|deploy|status|teardown|all}"
        echo ""
        echo "  setup     Create IAM user, register instance, create app/DG, upload revision"
        echo "  run       Start the agent in foreground"
        echo "  deploy    Trigger a deployment"
        echo "  status    Check last deployment status"
        echo "  teardown  Delete all AWS resources and local state"
        echo "  all       Full cycle: setup → run → deploy → status → teardown"
        ;;
esac
