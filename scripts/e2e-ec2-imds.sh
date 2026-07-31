#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# End-to-end test for the Rust CodeDeploy agent on EC2 — IMDS credentials.
#
# Launches an AL2023 EC2 instance with an instance profile, copies the
# agent binary to it via S3, and triggers a CodeDeploy deployment that
# the agent processes using IMDS credentials (no on-prem config file).
#
# Companion scripts cover the other EC2 credential modes:
#   - e2e-ec2-iam-user.sh      EC2 + on-prem registration, IamUser inline keys
#   - e2e-ec2-iam-session.sh   EC2 + on-prem registration, IamSession INI file
#
# Connectivity uses AWS SSM Session Manager (HTTPS) instead of SSH,
# so it works from restricted networks where port 22 is blocked.
#
# Usage:
#   ./scripts/e2e-ec2-imds.sh setup     # Create all AWS resources + launch EC2
#   ./scripts/e2e-ec2-imds.sh run       # Connect via SSM and run agent (foreground)
#   ./scripts/e2e-ec2-imds.sh deploy    # Trigger a deployment
#   ./scripts/e2e-ec2-imds.sh status    # Check deployment status
#   ./scripts/e2e-ec2-imds.sh teardown  # Delete all AWS resources
#   ./scripts/e2e-ec2-imds.sh all       # setup + run (background) + deploy + status + teardown
#
# Prerequisites:
#   - AWS credentials in the environment with admin-level access
#   - AWS CLI v2 installed
#   - Agent binary built: cargo build --release (or set AGENT_S3_URI / AGENT_S3_PREFIX)
#   - jq installed
#   - session-manager-plugin installed (for 'run' interactive mode)
#
# Optional: skip `cargo build --release` and pull a pre-built binary from S3.
#   AGENT_S3_URI    full s3://bucket/key (overrides AGENT_S3_PREFIX)
#   AGENT_S3_PREFIX s3://bucket/prefix; suffix /linux/codedeploy-agent is appended
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

# ── Configuration ────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
AGENT="${REPO_ROOT}/target/release/codedeploy-agent"

REMOTE_AGENT="/tmp/codedeploy-agent"
REMOTE_E2E_DIR="/tmp/codedeploy-agent-ec2-imds"
REMOTE_CONFIG="${REMOTE_E2E_DIR}/codedeployagent.yml"
REMOTE_LOG_DIR="${REMOTE_E2E_DIR}/logs"
REMOTE_PID_DIR="${REMOTE_E2E_DIR}/pid"
REMOTE_ROOT_DIR="${REMOTE_E2E_DIR}/deployment-root"

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
naming_init "ec2-imds"

check_prereqs() {
    ensure_local_agent linux "$AGENT"
    command -v aws >/dev/null || die "AWS CLI not found"
    command -v jq >/dev/null || die "jq not found"
    command -v zip >/dev/null || die "zip not found"
    if ! command -v session-manager-plugin >/dev/null 2>&1; then
        info "WARNING: session-manager-plugin not found — 'run' (interactive mode) will not work"
        info "  Install from: https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html"
    fi
    aws sts get-caller-identity >/dev/null 2>&1 || die "No valid AWS credentials in environment"
}

save_state() { echo "$1" | jq -S '.' > "$STATE_FILE"; }
load_state() { cat "$STATE_FILE" 2>/dev/null || echo '{}'; }

# Run a command on the EC2 instance via SSM send-command.
# Usage: ssm_cmd <instance_id> <region> <command_string>
ssm_cmd() {
    local instance_id="$1" region="$2"; shift 2
    local cmd_string="$*"

    local cmd_id
    cmd_id=$(aws ssm send-command \
        --instance-ids "$instance_id" \
        --document-name "AWS-RunShellScript" \
        --parameters "commands=[\"$cmd_string\"]" \
        --cloud-watch-output-config '{"CloudWatchOutputEnabled":true}' \
        --query 'Command.CommandId' --output text \
        --region "$region")

    local status=""
    for i in $(seq 1 60); do
        local invocation
        invocation=$(aws ssm get-command-invocation \
            --command-id "$cmd_id" \
            --instance-id "$instance_id" \
            --region "$region" 2>&1) || true

        status=$(echo "$invocation" | jq -r '.Status // empty' 2>/dev/null) || true

        case "$status" in
            Success)
                echo "$invocation" | jq -r '.StandardOutputContent // empty' 2>/dev/null
                return 0
                ;;
            Failed|TimedOut|Cancelled)
                local stderr_out
                stderr_out=$(echo "$invocation" | jq -r '.StandardErrorContent // empty' 2>/dev/null)
                die "SSM command failed (status=${status}): ${stderr_out}"
                ;;
            ""|InProgress|Pending|Delayed)
                sleep 2
                ;;
            *)
                sleep 2
                ;;
        esac
    done

    die "SSM command timed out after 120s waiting for completion (command_id=${cmd_id})"
}

# Wait for SSM agent on the instance to come online.
# Usage: wait_ssm_ready <instance_id> <region>
wait_ssm_ready() {
    local instance_id="$1" region="$2"

    info "Waiting for SSM agent to come online..."
    for i in $(seq 1 30); do
        local ping_status
        ping_status=$(aws ssm describe-instance-information \
            --filters "Key=InstanceIds,Values=${instance_id}" \
            --query 'InstanceInformationList[0].PingStatus' --output text \
            --region "$region" 2>/dev/null) || true

        if [[ "$ping_status" == "Online" ]]; then
            info "  SSM agent is online"
            return 0
        fi
        info "  Waiting for SSM agent... (${i}/30)"
        sleep 10
    done

    die "SSM agent did not come online after 5 minutes"
}

# ── Setup ────────────────────────────────────────────────────────────
cmd_setup() {
    check_prereqs
    info "Setting up EC2 e2e test resources in ${REGION}..."

    # 1. Create IAM role + instance profile for the EC2 agent.
    info "Creating IAM role: ${AGENT_ROLE_NAME}"
    if aws iam get-role --role-name "$AGENT_ROLE_NAME" --region "$REGION" >/dev/null 2>&1; then
        info "  IAM role already exists, reusing"
    else
        aws iam create-role \
            --role-name "$AGENT_ROLE_NAME" \
            --assume-role-policy-document '{
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "ec2.amazonaws.com"},
                    "Action": "sts:AssumeRole"
                }]
            }' \
            --region "$REGION"
    fi

    aws iam put-role-policy \
        --role-name "$AGENT_ROLE_NAME" \
        --policy-name "${AGENT_ROLE_POLICY_NAME}" \
        --policy-document '{
            "Version": "2012-10-17",
            "Statement": [
                {
                    "Effect": "Allow",
                    "Action": [
                        "codedeploy-commands:*",
                        "s3:GetObject",
                        "s3:GetObjectVersion",
                        "s3:ListBucket",
                        "s3:PutObject",
                        "s3:DeleteObject"
                    ],
                    "Resource": "*"
                },
                {
                    "Effect": "Allow",
                    "Action": [
                        "ssm:UpdateInstanceInformation",
                        "ssmmessages:CreateControlChannel",
                        "ssmmessages:CreateDataChannel",
                        "ssmmessages:OpenControlChannel",
                        "ssmmessages:OpenDataChannel",
                        "ec2messages:AcknowledgeMessage",
                        "ec2messages:DeleteMessage",
                        "ec2messages:FailMessage",
                        "ec2messages:GetEndpoint",
                        "ec2messages:GetMessages",
                        "ec2messages:SendReply"
                    ],
                    "Resource": "*"
                }
            ]
        }' \
        --region "$REGION"

    info "Creating instance profile: ${INSTANCE_PROFILE_NAME}"
    if aws iam get-instance-profile --instance-profile-name "$INSTANCE_PROFILE_NAME" --region "$REGION" >/dev/null 2>&1; then
        info "  Instance profile already exists, reusing"
    else
        aws iam create-instance-profile \
            --instance-profile-name "$INSTANCE_PROFILE_NAME" \
            --region "$REGION"
        aws iam add-role-to-instance-profile \
            --instance-profile-name "$INSTANCE_PROFILE_NAME" \
            --role-name "$AGENT_ROLE_NAME" \
            --region "$REGION" 2>/dev/null || true
    fi

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
        --policy-arn "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole" \
        --region "$REGION"

    SERVICE_ROLE_ARN=$(aws iam get-role --role-name "$SERVICE_ROLE_NAME" --query 'Role.Arn' --output text --region "$REGION")

    # 3. Launch EC2 instance.
    info "Looking up latest AL2023 AMI..."
    AMI_ID=$(aws ssm get-parameter \
        --name "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64" \
        --query 'Parameter.Value' --output text \
        --region "$REGION")
    info "  AMI: ${AMI_ID}"

    info "Waiting for instance profile to propagate..."
    sleep 10

    info "Launching EC2 instance..."
    INSTANCE_ID=$(aws ec2 run-instances \
        --image-id "$AMI_ID" \
        --instance-type t3.micro \
        --iam-instance-profile "Name=${INSTANCE_PROFILE_NAME}" \
        --metadata-options "HttpTokens=required,HttpPutResponseHopLimit=2,HttpEndpoint=enabled" \
        --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=${PREFIX}},{Key=${TAG_KEY},Value=${TAG_VALUE}}]" \
        --query 'Instances[0].InstanceId' --output text \
        --region "$REGION")
    info "  Instance: ${INSTANCE_ID}"

    info "Waiting for instance to be running..."
    aws ec2 wait instance-running --instance-ids "$INSTANCE_ID" --region "$REGION"

    info "Waiting for status checks..."
    aws ec2 wait instance-status-ok --instance-ids "$INSTANCE_ID" --region "$REGION"

    # 4. Wait for SSM and copy agent binary to instance via S3.
    wait_ssm_ready "$INSTANCE_ID" "$REGION"

    info "Uploading agent binary to S3..."
    if ! aws s3api head-bucket --bucket "$BUCKET_NAME" --region "$REGION" 2>/dev/null; then
        if [[ "$REGION" == "us-east-1" ]]; then
            aws s3api create-bucket --bucket "$BUCKET_NAME" --region "$REGION"
        else
            aws s3api create-bucket --bucket "$BUCKET_NAME" --region "$REGION" \
                --create-bucket-configuration "LocationConstraint=${REGION}"
        fi
    fi
    aws s3 cp "$AGENT" "s3://${BUCKET_NAME}/agent-binary" --region "$REGION"

    info "Downloading agent binary on instance..."
    ssm_cmd "$INSTANCE_ID" "$REGION" "aws s3 cp s3://${BUCKET_NAME}/agent-binary ${REMOTE_AGENT} --region ${REGION} && chmod +x ${REMOTE_AGENT}"

    # 5. Create remote directories and config.
    info "Setting up remote directories and config..."
    # chmod -R 777 so the agent works whether invoked via send-command (root) or start-session (ssm-user).
    ssm_cmd "$INSTANCE_ID" "$REGION" "mkdir -p ${REMOTE_LOG_DIR} ${REMOTE_PID_DIR} ${REMOTE_ROOT_DIR} && chmod -R 777 ${REMOTE_E2E_DIR}"
    ssm_cmd "$INSTANCE_ID" "$REGION" "cat > ${REMOTE_CONFIG} << 'EOFCONFIG'
verbose: true
wait_between_runs: 5
log_dir: ${REMOTE_LOG_DIR}
pid_dir: ${REMOTE_PID_DIR}
root_dir: ${REMOTE_ROOT_DIR}
EOFCONFIG"

    # 6. Create S3 bucket + sample revision.
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

    # 7. Create CodeDeploy application + deployment group.
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
            --ec2-tag-filters "Key=${TAG_KEY},Value=${TAG_VALUE},Type=KEY_AND_VALUE" \
            --service-role-arn "$SERVICE_ROLE_ARN" \
            --region "$REGION"
    else
        info "Waiting for IAM role to propagate..."
        sleep 10
        aws deploy create-deployment-group \
            --application-name "$APP_NAME" \
            --deployment-group-name "$DG_NAME" \
            --ec2-tag-filters "Key=${TAG_KEY},Value=${TAG_VALUE},Type=KEY_AND_VALUE" \
            --service-role-arn "$SERVICE_ROLE_ARN" \
            --region "$REGION"
    fi

    info "Waiting for IAM role to propagate..."
    sleep 15

    # 8. Save state.
    save_state "$(cat <<EOF
{
    "region": "${REGION}",
    "app_name": "${APP_NAME}",
    "dg_name": "${DG_NAME}",
    "instance_id": "${INSTANCE_ID}",
    "agent_role_name": "${AGENT_ROLE_NAME}",
    "instance_profile_name": "${INSTANCE_PROFILE_NAME}",
    "service_role_name": "${SERVICE_ROLE_NAME}",
    "service_role_arn": "${SERVICE_ROLE_ARN}",
    "bucket_name": "${BUCKET_NAME}"
}
EOF
)"

    info "Setup complete! State saved to ${STATE_FILE}"
    info ""
    info "Next steps:"
    info "  1. ./scripts/e2e-ec2-imds.sh run       # Launch the agent in the background on EC2"
    info "  2. ./scripts/e2e-ec2-imds.sh deploy     # Trigger a deployment (in another terminal)"
    info "  3. ./scripts/e2e-ec2-imds.sh status     # Check deployment status"
    info "  4. ./scripts/e2e-ec2-imds.sh logs    # Tail the agent log"
}

# ── Run agent (foreground, via SSM Session Manager) ──────────────────
cmd_run() {
    STATE=$(load_state)
    INSTANCE_ID=$(echo "$STATE" | jq -r '.instance_id')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')
    [[ "$INSTANCE_ID" != "null" ]] || die "No state found — run 'setup' first"

    if ! command -v session-manager-plugin >/dev/null 2>&1; then
        printf 'ERROR: session-manager-plugin not installed — `run` is interactive and requires it.\n  Install: https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html\n  Workaround: ./scripts/e2e-ec2-imds.sh all (runs the full cycle in the background, no plugin needed).\n' >&2
        exit 1
    fi

    info "Starting agent on ${INSTANCE_ID} in foreground via SSM (Ctrl-C to stop)..."
    info "Logs: use another SSM session or '$0 logs' to tail ${REMOTE_LOG_DIR}/codedeploy-agent*"
    aws ssm start-session --target "$INSTANCE_ID" --region "$REGION_STATE" \
        --document-name AWS-StartInteractiveCommand \
        --parameters command="CODEDEPLOY_DEVELOPER_MODE=true AWS_REGION=${REGION_STATE} ${REMOTE_AGENT} --config-file ${REMOTE_CONFIG} worker"
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

    DEPLOYMENT_ID=""
    for attempt in $(seq 1 12); do
        DEPLOY_OUTPUT=$(AWS_PAGER='' aws deploy create-deployment \
            --application-name "$APP" \
            --deployment-group-name "$DG" \
            --revision "revisionType=S3,s3Location={bucket=${BUCKET},key=revision.zip,bundleType=zip}" \
            --region "$REGION_STATE" 2>&1) || true

        if echo "$DEPLOY_OUTPUT" | grep -q "IAM_ROLE_PERMISSIONS"; then
            info "  Role not yet assumable (sync), retrying... (${attempt}/12)"
            sleep 10
            continue
        fi

        DEPLOYMENT_ID=$(echo "$DEPLOY_OUTPUT" | jq -r '.deploymentId // empty' 2>/dev/null)
        [[ -n "$DEPLOYMENT_ID" ]] || die "create-deployment failed: ${DEPLOY_OUTPUT}"

        sleep 3
        DEP_ERROR=$(aws deploy get-deployment --deployment-id "$DEPLOYMENT_ID" \
            --query 'deploymentInfo.errorInformation.code' --output text \
            --region "$REGION_STATE" 2>/dev/null) || true
        if [[ "$DEP_ERROR" == "IAM_ROLE_PERMISSIONS" ]]; then
            info "  Role not yet assumable (async), retrying... (${attempt}/12)"
            DEPLOYMENT_ID=""
            sleep 10
            continue
        fi

        break
    done

    [[ -n "$DEPLOYMENT_ID" ]] || die "Gave up waiting for IAM role propagation after 12 attempts"

    info "Deployment created: ${DEPLOYMENT_ID}"
    info "Watch progress: ./scripts/e2e-ec2-imds.sh status"

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
        print_banner "TIMEOUT" "EC2 E2E — deployment did not reach terminal state in 10 min"
        die "Timeout"
    fi

    info "Final status:"
    print_deployment_table "$REGION_STATE" "$DEPLOYMENT_ID"

    if (( DEPLOY_SUCCEEDED == DEPLOY_TOTAL )); then
        print_banner "PASS" "EC2 E2E — deployment Succeeded" \
            "Deployment: ${DEPLOYMENT_ID}" \
            "Elapsed: ${DEPLOY_ELAPSED}s"
        print_deployment_outcome SUCCEEDED
    else
        print_banner "FAIL" "EC2 E2E — deployment did not succeed" \
            "Deployment: ${DEPLOYMENT_ID}" \
            "Succeeded: ${DEPLOY_SUCCEEDED}  Failed: ${DEPLOY_FAILED}  Stopped: ${DEPLOY_STOPPED}" \
            "Elapsed: ${DEPLOY_ELAPSED}s"
        print_deployment_outcome FAILED
        return 1
    fi
}

# ── Logs ─────────────────────────────────────────────────────────────
# Tail the agent log via SSM send-command (no plugin needed).
cmd_logs() {
    STATE=$(load_state)
    INSTANCE_ID=$(echo "$STATE" | jq -r '.instance_id')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')
    [[ "$INSTANCE_ID" != "null" ]] || die "No state found — run 'setup' first"

    info "Agent log (last 50 lines from ${INSTANCE_ID}):"
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "tail -n 50 ${REMOTE_LOG_DIR}/codedeploy-agent* 2>/dev/null" || true
}

# ── Teardown ─────────────────────────────────────────────────────────
cmd_teardown() {
    info "Tearing down EC2 e2e test resources..."
    STATE=$(load_state)
    REGION_STATE=$(echo "$STATE" | jq -r '.region // "us-east-1"')
    APP=$(echo "$STATE" | jq -r '.app_name // empty')
    INSTANCE_ID=$(echo "$STATE" | jq -r '.instance_id // empty')
    AGENT_ROLE=$(echo "$STATE" | jq -r '.agent_role_name // empty')
    PROFILE=$(echo "$STATE" | jq -r '.instance_profile_name // empty')
    ROLE=$(echo "$STATE" | jq -r '.service_role_name // empty')
    BUCKET=$(echo "$STATE" | jq -r '.bucket_name // empty')

    if [[ -n "$INSTANCE_ID" ]]; then
        info "Killing agent on instance..."
        ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "pkill -f codedeploy-agent" 2>/dev/null || true
    fi

    if [[ -n "$INSTANCE_ID" ]]; then
        info "Terminating instance: ${INSTANCE_ID}"
        aws ec2 terminate-instances --instance-ids "$INSTANCE_ID" --region "$REGION_STATE" 2>/dev/null || true
        info "Waiting for instance to terminate..."
        aws ec2 wait instance-terminated --instance-ids "$INSTANCE_ID" --region "$REGION_STATE" 2>/dev/null || true
    fi

    if [[ -n "$PROFILE" ]]; then
        info "Deleting instance profile: ${PROFILE}"
        aws iam remove-role-from-instance-profile \
            --instance-profile-name "$PROFILE" \
            --role-name "${AGENT_ROLE:-${AGENT_ROLE_NAME}}" \
            --region "$REGION_STATE" 2>/dev/null || true
        aws iam delete-instance-profile \
            --instance-profile-name "$PROFILE" \
            --region "$REGION_STATE" 2>/dev/null || true
    fi
    if [[ -n "$AGENT_ROLE" ]]; then
        info "Deleting agent role: ${AGENT_ROLE}"
        aws iam delete-role-policy --role-name "$AGENT_ROLE" --policy-name "${AGENT_ROLE_POLICY_NAME}" --region "$REGION_STATE" 2>/dev/null || true
        aws iam delete-role --role-name "$AGENT_ROLE" --region "$REGION_STATE" 2>/dev/null || true
    fi

    if [[ -n "$APP" ]]; then
        info "Deleting application: ${APP}"
        aws deploy delete-application \
            --application-name "$APP" \
            --region "$REGION_STATE" 2>/dev/null || true
    fi

    if [[ -n "$ROLE" ]]; then
        info "Deleting service role: ${ROLE}"
        aws iam detach-role-policy --role-name "$ROLE" --policy-arn "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole" 2>/dev/null || true
        aws iam delete-role-policy --role-name "$ROLE" --policy-name "${AGENT_ROLE_POLICY_NAME}" 2>/dev/null || true
        aws iam delete-role --role-name "$ROLE" 2>/dev/null || true
    fi

    if [[ -n "$BUCKET" ]]; then
        info "Deleting S3 bucket: ${BUCKET}"
        aws s3 rm "s3://${BUCKET}" --recursive --region "$REGION_STATE" 2>/dev/null || true
        aws s3api delete-bucket --bucket "$BUCKET" --region "$REGION_STATE" 2>/dev/null || true
    fi

    rm -f "$STATE_FILE"

    info "Teardown complete."
}

# ── All (full cycle) ─────────────────────────────────────────────────
cmd_all() {
    trap 'info "Error detected, running teardown..."; replay_deployment_outcome; cmd_teardown' ERR
    cmd_setup

    STATE=$(load_state)
    INSTANCE_ID=$(echo "$STATE" | jq -r '.instance_id')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')

    info "Starting agent in background on EC2..."
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "nohup bash -c 'CODEDEPLOY_DEVELOPER_MODE=true AWS_REGION=${REGION_STATE} ${REMOTE_AGENT} --config-file ${REMOTE_CONFIG} worker' > ${REMOTE_E2E_DIR}/agent.out 2>&1 &"
    sleep 5
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "pgrep -f '[c]odedeploy-agent'" >/dev/null || die "Agent failed to start on EC2"

    cmd_deploy
    info "Waiting 30s for deployment to process..."
    sleep 30
    cmd_status || true

    info "Agent logs (tail):"
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "tail -n 50 ${REMOTE_LOG_DIR}/codedeploy-agent* 2>/dev/null" || true

    replay_deployment_outcome
    cmd_teardown
}

# ── Main ─────────────────────────────────────────────────────────────
case "${1:-help}" in
    setup)    cmd_setup ;;
    run)      cmd_run ;;
    deploy)   cmd_deploy ;;
    status)   cmd_status ;;
    logs)     cmd_logs ;;
    teardown) cmd_teardown ;;
    all)      cmd_all ;;
    help|*)
        echo "Usage: $0 {setup|run|deploy|status|logs|teardown|all}"
        echo ""
        echo "  setup     Create IAM role, launch EC2, copy agent (via S3+SSM), create app/DG, upload revision"
        echo "  run       Start the agent on EC2 in foreground (SSM Session Manager)"
        echo "  deploy    Trigger a deployment"
        echo "  status    Check last deployment status"
        echo "  logs      Tail the agent log on EC2 (via SSM send-command)"
        echo "  teardown  Delete all AWS resources and local state"
        echo "  all       Full cycle: setup → run → deploy → status → logs → teardown"
        ;;
esac
