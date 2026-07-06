#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# End-to-end test for the Rust CodeDeploy agent on EC2 — IamSession mode.
#
# Launches an AL2023 EC2 instance, creates a dedicated IAM user, registers
# the instance as an on-premises instance, then runs the agent on the
# instance with an `on_premises_config_file` that contains `iam_session_arn`
# + `aws_credentials_file` pointing at a separate INI credentials file.
# This exercises the file_credentials.rs code path.
#
# The instance profile only carries permissions needed to fetch the agent
# binary from S3 and to talk to SSM — IMDS is NOT used for CodeDeploy API
# calls, that's what the on-prem credentials are for.
#
# Companion scripts cover the other EC2 credential modes:
#   - e2e-ec2-imds.sh         EC2 + IMDS (instance profile only)
#   - e2e-ec2-iam-user.sh     EC2 + on-prem registration, IamUser inline keys
#
# Connectivity uses AWS SSM Session Manager (HTTPS) instead of SSH,
# so it works from restricted networks where port 22 is blocked.
#
# Usage:
#   ./scripts/e2e-ec2-iam-session.sh setup     # Create all AWS resources + launch EC2
#   ./scripts/e2e-ec2-iam-session.sh run       # Connect via SSM and run agent (foreground)
#   ./scripts/e2e-ec2-iam-session.sh deploy    # Trigger a deployment
#   ./scripts/e2e-ec2-iam-session.sh status    # Check deployment status
#   ./scripts/e2e-ec2-iam-session.sh teardown  # Delete all AWS resources
#   ./scripts/e2e-ec2-iam-session.sh all       # setup + run (background) + deploy + status + teardown
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
REMOTE_E2E_DIR="/tmp/codedeploy-agent-ec2-iam-session"
REMOTE_CONFIG="${REMOTE_E2E_DIR}/codedeployagent.yml"
REMOTE_ONPREM_CONFIG="${REMOTE_E2E_DIR}/codedeploy.onpremises.yml"
REMOTE_CREDENTIALS_FILE="${REMOTE_E2E_DIR}/credentials"
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
naming_init "ec2-iam-session"

check_prereqs() {
    ensure_local_agent linux "$AGENT"
    command -v aws >/dev/null || die "AWS CLI not found"
    command -v jq >/dev/null || die "jq not found"
    command -v zip >/dev/null || die "zip not found"
    if ! command -v session-manager-plugin >/dev/null 2>&1; then
        info "WARNING: session-manager-plugin not found — 'run' (interactive mode) will not work"
    fi
    aws sts get-caller-identity >/dev/null 2>&1 || die "No valid AWS credentials in environment"
}

save_state() { echo "$1" | jq -S '.' > "$STATE_FILE"; }
load_state() { cat "$STATE_FILE" 2>/dev/null || echo '{}'; }

# Run a command on the EC2 instance via SSM send-command.
ssm_cmd() {
    local instance_id="$1" region="$2"; shift 2
    local cmd_string="$*"
    local cmd_id
    cmd_id=$(aws ssm send-command \
        --instance-ids "$instance_id" \
        --document-name "AWS-RunShellScript" \
        --parameters "commands=[\"$cmd_string\"]" \
        --query 'Command.CommandId' --output text \
        --region "$region")

    local status=""
    for _ in $(seq 1 60); do
        local invocation
        invocation=$(aws ssm get-command-invocation \
            --command-id "$cmd_id" \
            --instance-id "$instance_id" \
            --region "$region" 2>&1) || true
        status=$(echo "$invocation" | jq -r '.Status // empty' 2>/dev/null) || true
        case "$status" in
            Success)
                echo "$invocation" | jq -r '.StandardOutputContent // empty' 2>/dev/null
                return 0 ;;
            Failed|TimedOut|Cancelled)
                local stderr_out
                stderr_out=$(echo "$invocation" | jq -r '.StandardErrorContent // empty' 2>/dev/null)
                die "SSM command failed (status=${status}): ${stderr_out}" ;;
            *) sleep 2 ;;
        esac
    done
    die "SSM command timed out after 120s (command_id=${cmd_id})"
}

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
    info "Setting up EC2 + IamSession e2e test resources in ${REGION}..."
    info "  Credential mode: IamSession (on-prem registration, NOT IMDS)"

    # 1. IAM user that owns the on-prem instance + holds inline access keys.
    info "Creating IAM user: ${AGENT_USER}"
    if aws iam get-user --user-name "$AGENT_USER" >/dev/null 2>&1; then
        info "  IAM user already exists, reusing"
    else
        aws iam create-user --user-name "$AGENT_USER" >/dev/null
    fi
    aws iam put-user-policy \
        --user-name "$AGENT_USER" \
        --policy-name "${AGENT_USER_POLICY_NAME}" \
        --policy-document '{
            "Version":"2012-10-17",
            "Statement":[{
                "Effect":"Allow",
                "Action":[
                    "codedeploy-commands:*",
                    "s3:GetObject","s3:GetObjectVersion","s3:ListBucket"
                ],
                "Resource":"*"
            }]
        }' >/dev/null

    # Rotate access keys.
    EXISTING_KEYS=$(aws iam list-access-keys --user-name "$AGENT_USER" --query 'AccessKeyMetadata[].AccessKeyId' --output text)
    for key in $EXISTING_KEYS; do
        info "  Deleting stale access key: ${key}"
        aws iam delete-access-key --user-name "$AGENT_USER" --access-key-id "$key"
    done
    KEYS=$(aws iam create-access-key --user-name "$AGENT_USER" --output json)
    ACCESS_KEY=$(echo "$KEYS" | jq -r '.AccessKey.AccessKeyId')
    SECRET_KEY=$(echo "$KEYS" | jq -r '.AccessKey.SecretAccessKey')
    IAM_USER_ARN=$(aws iam get-user --user-name "$AGENT_USER" --query 'User.Arn' --output text)
    info "  Agent access key: ${ACCESS_KEY}"

    # 2. Minimal instance role — S3 + SSM only; CodeDeploy calls use the on-prem IAM user, not IMDS.
    info "Creating EC2 instance role: ${AGENT_ROLE_NAME}"
    if ! aws iam get-role --role-name "$AGENT_ROLE_NAME" >/dev/null 2>&1; then
        aws iam create-role \
            --role-name "$AGENT_ROLE_NAME" \
            --assume-role-policy-document '{
                "Version":"2012-10-17",
                "Statement":[{
                    "Effect":"Allow",
                    "Principal":{"Service":"ec2.amazonaws.com"},
                    "Action":"sts:AssumeRole"
                }]
            }' >/dev/null
    fi
    aws iam put-role-policy \
        --role-name "$AGENT_ROLE_NAME" \
        --policy-name "${AGENT_ROLE_POLICY_NAME}" \
        --policy-document '{
            "Version":"2012-10-17",
            "Statement":[
                {
                    "Effect":"Allow",
                    "Action":["s3:GetObject","s3:GetObjectVersion","s3:ListBucket"],
                    "Resource":"*"
                },
                {
                    "Effect":"Allow",
                    "Action":[
                        "ssm:UpdateInstanceInformation",
                        "ssmmessages:CreateControlChannel",
                        "ssmmessages:CreateDataChannel",
                        "ssmmessages:OpenControlChannel",
                        "ssmmessages:OpenDataChannel",
                        "ec2messages:GetMessages",
                        "ec2messages:AcknowledgeMessage",
                        "ec2messages:SendReply"
                    ],
                    "Resource":"*"
                }
            ]
        }' >/dev/null

    if ! aws iam get-instance-profile --instance-profile-name "$INSTANCE_PROFILE_NAME" >/dev/null 2>&1; then
        aws iam create-instance-profile --instance-profile-name "$INSTANCE_PROFILE_NAME" >/dev/null
        aws iam add-role-to-instance-profile --instance-profile-name "$INSTANCE_PROFILE_NAME" --role-name "$AGENT_ROLE_NAME"
    fi

    # 3. CodeDeploy service role.
    info "Creating CodeDeploy service role: ${SERVICE_ROLE_NAME}"
    if ! aws iam get-role --role-name "$SERVICE_ROLE_NAME" >/dev/null 2>&1; then
        aws iam create-role \
            --role-name "$SERVICE_ROLE_NAME" \
            --assume-role-policy-document '{
                "Version":"2012-10-17",
                "Statement":[{
                    "Effect":"Allow",
                    "Principal":{"Service":"codedeploy.amazonaws.com"},
                    "Action":"sts:AssumeRole"
                }]
            }' >/dev/null
    fi
    aws iam attach-role-policy \
        --role-name "$SERVICE_ROLE_NAME" \
        --policy-arn "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole"
    SERVICE_ROLE_ARN=$(aws iam get-role --role-name "$SERVICE_ROLE_NAME" --query 'Role.Arn' --output text)

    # 4. Launch EC2 instance.
    info "Looking up latest AL2023 AMI..."
    AMI_ID=$(aws ssm get-parameter \
        --name "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64" \
        --query 'Parameter.Value' --output text --region "$REGION")
    info "  AMI: ${AMI_ID}"

    info "Waiting for instance profile to propagate..."
    sleep 10

    info "Launching EC2 instance..."
    INSTANCE_ID=$(aws ec2 run-instances \
        --image-id "$AMI_ID" \
        --instance-type t3.micro \
        --iam-instance-profile "Name=${INSTANCE_PROFILE_NAME}" \
        --metadata-options "HttpTokens=required,HttpPutResponseHopLimit=2,HttpEndpoint=enabled" \
        --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=${PREFIX}}]" \
        --query 'Instances[0].InstanceId' --output text --region "$REGION")
    info "  Instance: ${INSTANCE_ID}"

    info "Waiting for instance running + status checks..."
    aws ec2 wait instance-running --instance-ids "$INSTANCE_ID" --region "$REGION"
    aws ec2 wait instance-status-ok --instance-ids "$INSTANCE_ID" --region "$REGION"
    wait_ssm_ready "$INSTANCE_ID" "$REGION"

    # 5. Register instance as on-prem so the agent uses IamSession credentials, not IMDS.
    INSTANCE_NAME="${PREFIX}-${INSTANCE_ID}"
    info "Registering on-premises instance: ${INSTANCE_NAME}"
    aws deploy register-on-premises-instance \
        --instance-name "$INSTANCE_NAME" \
        --iam-user-arn "$IAM_USER_ARN" \
        --region "$REGION" 2>/dev/null || true
    aws deploy add-tags-to-on-premises-instances \
        --instance-names "$INSTANCE_NAME" \
        --tags "Key=${TAG_KEY},Value=${TAG_VALUE}" \
        --region "$REGION" 2>/dev/null || true

    # 6. S3 bucket + agent binary upload.
    info "Creating S3 bucket: ${BUCKET_NAME}"
    if ! aws s3api head-bucket --bucket "$BUCKET_NAME" --region "$REGION" 2>/dev/null; then
        if [[ "$REGION" == "us-east-1" ]]; then
            aws s3api create-bucket --bucket "$BUCKET_NAME" --region "$REGION" >/dev/null
        else
            aws s3api create-bucket --bucket "$BUCKET_NAME" --region "$REGION" \
                --create-bucket-configuration "LocationConstraint=${REGION}" >/dev/null
        fi
    fi
    info "Uploading agent binary to S3..."
    aws s3 cp "$AGENT" "s3://${BUCKET_NAME}/agent-binary" --region "$REGION" >/dev/null

    info "Staging agent + on-prem config on instance..."
    ssm_cmd "$INSTANCE_ID" "$REGION" "aws s3 cp s3://${BUCKET_NAME}/agent-binary ${REMOTE_AGENT} --region ${REGION} && chmod +x ${REMOTE_AGENT}"
    ssm_cmd "$INSTANCE_ID" "$REGION" "mkdir -p ${REMOTE_LOG_DIR} ${REMOTE_PID_DIR} ${REMOTE_ROOT_DIR} && chmod -R 777 ${REMOTE_E2E_DIR}"
    ssm_cmd "$INSTANCE_ID" "$REGION" "cat > ${REMOTE_CONFIG} << 'EOFCONFIG'
verbose: true
wait_between_runs: 5
log_dir: ${REMOTE_LOG_DIR}
pid_dir: ${REMOTE_PID_DIR}
root_dir: ${REMOTE_ROOT_DIR}
on_premises_config_file: ${REMOTE_ONPREM_CONFIG}
EOFCONFIG"
    # On-prem config — IamSession mode: iam_session_arn + aws_credentials_file (no inline keys).
    ssm_cmd "$INSTANCE_ID" "$REGION" "cat > ${REMOTE_ONPREM_CONFIG} << 'EOFONPREM'
region: ${REGION}
iam_session_arn: ${IAM_USER_ARN}
aws_credentials_file: ${REMOTE_CREDENTIALS_FILE}
EOFONPREM
chmod 600 ${REMOTE_ONPREM_CONFIG}"
    # Separate INI credentials file read by file_credentials.rs.
    ssm_cmd "$INSTANCE_ID" "$REGION" "cat > ${REMOTE_CREDENTIALS_FILE} << 'EOFCREDS'
[default]
aws_access_key_id = ${ACCESS_KEY}
aws_secret_access_key = ${SECRET_KEY}
EOFCREDS
chmod 600 ${REMOTE_CREDENTIALS_FILE}"

    # 7. Sample revision.
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
echo "Deployment successful! (EC2 + IamSession credential path)"
SCRIPT
    chmod +x "${REVISION_DIR}/scripts/after_install.sh"
    REVISION_ZIP="/tmp/${PREFIX}-revision.zip"
    (cd "$REVISION_DIR" && zip -r "$REVISION_ZIP" . >/dev/null)
    aws s3 cp "$REVISION_ZIP" "s3://${BUCKET_NAME}/revision.zip" --region "$REGION" >/dev/null
    rm -rf "$REVISION_DIR" "$REVISION_ZIP"

    # 8. CodeDeploy app + deployment group targeting the on-prem instance.
    info "Creating application: ${APP_NAME}"
    if ! aws deploy get-application --application-name "$APP_NAME" --region "$REGION" >/dev/null 2>&1; then
        aws deploy create-application --application-name "$APP_NAME" --compute-platform Server --region "$REGION" >/dev/null
    fi

    info "Waiting for IAM role to propagate before creating deployment group..."
    sleep 10

    info "Creating deployment group: ${DG_NAME}"
    if aws deploy get-deployment-group --application-name "$APP_NAME" --deployment-group-name "$DG_NAME" --region "$REGION" >/dev/null 2>&1; then
        aws deploy update-deployment-group \
            --application-name "$APP_NAME" \
            --current-deployment-group-name "$DG_NAME" \
            --on-premises-instance-tag-filters "Key=${TAG_KEY},Value=${TAG_VALUE},Type=KEY_AND_VALUE" \
            --service-role-arn "$SERVICE_ROLE_ARN" \
            --region "$REGION" >/dev/null
    else
        aws deploy create-deployment-group \
            --application-name "$APP_NAME" \
            --deployment-group-name "$DG_NAME" \
            --on-premises-instance-tag-filters "Key=${TAG_KEY},Value=${TAG_VALUE},Type=KEY_AND_VALUE" \
            --service-role-arn "$SERVICE_ROLE_ARN" \
            --region "$REGION" >/dev/null
    fi

    info "Waiting for IAM role to propagate..."
    sleep 15

    save_state "$(jq -n \
        --arg region "$REGION" \
        --arg app "$APP_NAME" \
        --arg dg "$DG_NAME" \
        --arg instance "$INSTANCE_ID" \
        --arg instance_name "$INSTANCE_NAME" \
        --arg agent_role "$AGENT_ROLE_NAME" \
        --arg profile "$INSTANCE_PROFILE_NAME" \
        --arg svc_role "$SERVICE_ROLE_NAME" \
        --arg svc_arn "$SERVICE_ROLE_ARN" \
        --arg agent_user "$AGENT_USER" \
        --arg access_key "$ACCESS_KEY" \
        --arg iam_user_arn "$IAM_USER_ARN" \
        --arg bucket "$BUCKET_NAME" \
        '{region:$region, app_name:$app, dg_name:$dg, instance_id:$instance,
          instance_name:$instance_name, agent_role_name:$agent_role,
          instance_profile_name:$profile, service_role_name:$svc_role,
          service_role_arn:$svc_arn, agent_user:$agent_user,
          access_key_id:$access_key, iam_user_arn:$iam_user_arn,
          bucket_name:$bucket}')"

    info "Setup complete. State: ${STATE_FILE}"
    info ""
    info "Next steps:"
    info "  1. ./scripts/e2e-ec2-iam-session.sh run     # Start the agent on EC2 in foreground (SSM Session Manager)"
    info "  2. ./scripts/e2e-ec2-iam-session.sh deploy  # Trigger a deployment (in another terminal)"
    info "  3. ./scripts/e2e-ec2-iam-session.sh status  # Check deployment status"
    info "  4. ./scripts/e2e-ec2-iam-session.sh logs    # Tail the agent log"
}

# ── Run agent (foreground, via SSM Session Manager) ─────────────────
cmd_run() {
    STATE=$(load_state)
    INSTANCE_ID=$(echo "$STATE" | jq -r '.instance_id')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')
    [[ "$INSTANCE_ID" != "null" ]] || die "No state — run 'setup' first"

    if ! command -v session-manager-plugin >/dev/null 2>&1; then
        printf 'ERROR: session-manager-plugin not installed — `run` is interactive and requires it.\n  Install: https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html\n  Workaround: ./scripts/e2e-ec2-iam-session.sh all (runs the full cycle in the background, no plugin needed).\n' >&2
        exit 1
    fi

    info "Starting agent on ${INSTANCE_ID} via SSM (Ctrl-C to stop)..."
    info "Logs: use another SSM session or '$0 logs' to tail ${REMOTE_LOG_DIR}/codedeploy-agent*"
    aws ssm start-session --target "$INSTANCE_ID" --region "$REGION_STATE" \
        --document-name AWS-StartInteractiveCommand \
        --parameters command="CODEDEPLOY_DEVELOPER_MODE=true AWS_REGION=${REGION_STATE} ${REMOTE_AGENT} --config-file ${REMOTE_CONFIG} worker"
}

# ── Deploy ──────────────────────────────────────────────────────────
cmd_deploy() {
    check_prereqs
    STATE=$(load_state)
    APP=$(echo "$STATE" | jq -r '.app_name')
    DG=$(echo "$STATE" | jq -r '.dg_name')
    BUCKET=$(echo "$STATE" | jq -r '.bucket_name')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')
    [[ "$APP" != "null" ]] || die "No state — run 'setup' first"

    info "Creating deployment for ${APP} / ${DG}..."

    DEPLOYMENT_ID=""
    for attempt in $(seq 1 12); do
        # Force no pager so create-deployment never blocks on `less`.
        DEPLOY_OUTPUT=$(AWS_PAGER='' aws deploy create-deployment \
            --application-name "$APP" \
            --deployment-group-name "$DG" \
            --revision "revisionType=S3,s3Location={bucket=${BUCKET},key=revision.zip,bundleType=zip}" \
            --region "$REGION_STATE" 2>&1) || true
        info "  attempt ${attempt}/12: $(echo "$DEPLOY_OUTPUT" | head -c 120 | tr '\n' ' ')"

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
    [[ -n "$DEPLOYMENT_ID" ]] || die "Gave up waiting for IAM role propagation"

    info "Deployment created: ${DEPLOYMENT_ID}"
    info "Watch progress: ./scripts/e2e-ec2-iam-session.sh status"

    echo "$STATE" | jq --arg id "$DEPLOYMENT_ID" '.last_deployment_id = $id' > "$STATE_FILE"
}

# ── Status ──────────────────────────────────────────────────────────
cmd_status() {
    check_prereqs
    STATE=$(load_state)
    DEPLOYMENT_ID=$(echo "$STATE" | jq -r '.last_deployment_id // empty')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')
    [[ -n "$DEPLOYMENT_ID" ]] || die "No deployment — run 'deploy' first"

    info "Polling deployment ${DEPLOYMENT_ID}..."
    if ! wait_for_deployments "$REGION_STATE" 600 "$DEPLOYMENT_ID"; then
        print_banner "TIMEOUT" "EC2 + IamSession E2E — deployment did not reach terminal state in 10 min"
        die "Timeout"
    fi

    info "Final status:"
    print_deployment_table "$REGION_STATE" "$DEPLOYMENT_ID"

    if (( DEPLOY_SUCCEEDED == DEPLOY_TOTAL )); then
        print_banner "PASS" "EC2 + IamSession E2E — deployment Succeeded" \
            "Deployment: ${DEPLOYMENT_ID}" \
            "Credential mode: IamSession (on-prem registration, NOT IMDS)" \
            "Elapsed: ${DEPLOY_ELAPSED}s"
        print_deployment_outcome SUCCEEDED
    else
        print_banner "FAIL" "EC2 + IamSession E2E — deployment did not succeed" \
            "Deployment: ${DEPLOYMENT_ID}" \
            "Succeeded: ${DEPLOY_SUCCEEDED}  Failed: ${DEPLOY_FAILED}  Stopped: ${DEPLOY_STOPPED}" \
            "Elapsed: ${DEPLOY_ELAPSED}s"
        print_deployment_outcome FAILED
        return 1
    fi
}

# ── Logs ────────────────────────────────────────────────────────────
# Tail the agent log via SSM send-command (no plugin needed).
cmd_logs() {
    STATE=$(load_state)
    INSTANCE_ID=$(echo "$STATE" | jq -r '.instance_id')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')
    [[ "$INSTANCE_ID" != "null" ]] || die "No state — run 'setup' first"

    info "Agent log (last 50 lines from ${INSTANCE_ID}):"
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "tail -n 50 ${REMOTE_LOG_DIR}/codedeploy-agent* 2>/dev/null" || true
}

# ── Teardown ────────────────────────────────────────────────────────
cmd_teardown() {
    info "Tearing down EC2 + IamSession test resources..."
    STATE=$(load_state)
    REGION_STATE=$(echo "$STATE"   | jq -r '.region // "us-east-1"')
    APP=$(echo "$STATE"            | jq -r '.app_name // empty')
    INSTANCE_ID=$(echo "$STATE"    | jq -r '.instance_id // empty')
    INSTANCE_NAME=$(echo "$STATE"  | jq -r '.instance_name // empty')
    AGENT_ROLE=$(echo "$STATE"     | jq -r '.agent_role_name // empty')
    PROFILE=$(echo "$STATE"        | jq -r '.instance_profile_name // empty')
    ROLE=$(echo "$STATE"           | jq -r '.service_role_name // empty')
    USER_NAME=$(echo "$STATE"      | jq -r '.agent_user // empty')
    ACCESS_KEY_ID=$(echo "$STATE"  | jq -r '.access_key_id // empty')
    BUCKET=$(echo "$STATE"         | jq -r '.bucket_name // empty')

    if [[ -n "$INSTANCE_ID" ]]; then
        info "Killing agent on instance + terminating: ${INSTANCE_ID}"
        ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "pkill -f codedeploy-agent" 2>/dev/null || true
        aws ec2 terminate-instances --instance-ids "$INSTANCE_ID" --region "$REGION_STATE" >/dev/null 2>&1 || true
        aws ec2 wait instance-terminated --instance-ids "$INSTANCE_ID" --region "$REGION_STATE" 2>/dev/null || true
    fi

    if [[ -n "$INSTANCE_NAME" ]]; then
        info "Deregistering on-premises instance: ${INSTANCE_NAME}"
        aws deploy remove-tags-from-on-premises-instances \
            --instance-names "$INSTANCE_NAME" \
            --tags "Key=${TAG_KEY},Value=${TAG_VALUE}" \
            --region "$REGION_STATE" 2>/dev/null || true
        aws deploy deregister-on-premises-instance \
            --instance-name "$INSTANCE_NAME" \
            --region "$REGION_STATE" 2>/dev/null || true
    fi

    if [[ -n "$PROFILE" ]]; then
        aws iam remove-role-from-instance-profile \
            --instance-profile-name "$PROFILE" \
            --role-name "${AGENT_ROLE:-${PREFIX}-agent-role}" 2>/dev/null || true
        aws iam delete-instance-profile --instance-profile-name "$PROFILE" 2>/dev/null || true
    fi
    if [[ -n "$AGENT_ROLE" ]]; then
        aws iam delete-role-policy --role-name "$AGENT_ROLE" --policy-name "${AGENT_ROLE_POLICY_NAME}" 2>/dev/null || true
        aws iam delete-role --role-name "$AGENT_ROLE" 2>/dev/null || true
    fi

    if [[ -n "$USER_NAME" ]]; then
        info "Deleting IAM user: ${USER_NAME}"
        if [[ -n "$ACCESS_KEY_ID" ]]; then
            aws iam delete-access-key --user-name "$USER_NAME" --access-key-id "$ACCESS_KEY_ID" 2>/dev/null || true
        fi
        aws iam delete-user-policy --user-name "$USER_NAME" --policy-name "${AGENT_USER_POLICY_NAME}" 2>/dev/null || true
        aws iam delete-user --user-name "$USER_NAME" 2>/dev/null || true
    fi

    if [[ -n "$APP" ]]; then
        aws deploy delete-application --application-name "$APP" --region "$REGION_STATE" 2>/dev/null || true
    fi
    if [[ -n "$ROLE" ]]; then
        aws iam detach-role-policy --role-name "$ROLE" --policy-arn "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole" 2>/dev/null || true
        aws iam delete-role --role-name "$ROLE" 2>/dev/null || true
    fi
    if [[ -n "$BUCKET" ]]; then
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
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" \
        "nohup bash -c 'CODEDEPLOY_DEVELOPER_MODE=true AWS_REGION=${REGION_STATE} ${REMOTE_AGENT} --config-file ${REMOTE_CONFIG} worker' > ${REMOTE_E2E_DIR}/agent.out 2>&1 &"
    sleep 5
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "pgrep -f '[c]odedeploy-agent'" >/dev/null || die "Agent failed to start on EC2"

    cmd_deploy
    cmd_status || true

    info "Agent logs (tail):"
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "tail -n 50 ${REMOTE_LOG_DIR}/codedeploy-agent* 2>/dev/null" || true

    replay_deployment_outcome
    cmd_teardown
}

# ── Main ────────────────────────────────────────────────────────────
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
        echo "  Tests the IamSession credential path on an EC2 instance"
        echo "  (instance is registered as on-prem; IMDS is NOT used for CodeDeploy)."
        echo ""
        echo "  setup     Create IAM user + role, launch EC2, register as on-prem, create app/DG"
        echo "  run       Start the agent on EC2 in foreground (SSM Session Manager)"
        echo "  deploy    Trigger a deployment"
        echo "  status    Check last deployment status"
        echo "  logs      Tail the agent log on EC2 (via SSM send-command)"
        echo "  teardown  Terminate instance + delete all AWS resources"
        echo "  all       Full cycle: setup → run → deploy → status → logs → teardown"
        ;;
esac
