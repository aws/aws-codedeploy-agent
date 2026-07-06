#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# Concurrent-deployment E2E test for the Rust CodeDeploy agent on EC2.
#
# Launches a single AL2023 EC2 instance running the agent, creates
# CONCURRENCY (default 3) CodeDeploy deployment groups that all target
# that same instance via tag, then triggers all CONCURRENCY deployments
# in parallel. The agent's 16-thread command pool processes them
# concurrently. Each deployment's AfterInstall hook sleeps 15s so the
# executions visibly overlap in the agent's scripts.log.
#
# Connectivity uses AWS SSM Session Manager (HTTPS) instead of SSH,
# so it works from restricted networks where port 22 is blocked.
#
# Usage:
#   CONCURRENCY=5 ./scripts/e2e-ec2-concurrent.sh setup
#   ./scripts/e2e-ec2-concurrent.sh run        # foreground agent (one terminal)
#   ./scripts/e2e-ec2-concurrent.sh deploy     # trigger N parallel deployments
#   ./scripts/e2e-ec2-concurrent.sh status     # wait + assert all succeeded
#   ./scripts/e2e-ec2-concurrent.sh logs       # fetch and grep agent logs for overlap
#   ./scripts/e2e-ec2-concurrent.sh teardown   # delete all AWS resources
#   ./scripts/e2e-ec2-concurrent.sh all        # setup + run (background) + deploy + status + logs + teardown
#
# Prerequisites:
#   - AWS credentials in the environment with admin-level access
#   - AWS CLI v2 installed
#   - Agent binary built: cargo build --release
#   - jq installed
#   - session-manager-plugin installed (for 'run' interactive mode)
#
# Optional: skip `cargo build --release` and pull a pre-built binary from S3.
#   AGENT_S3_URI    full s3://bucket/key (overrides AGENT_S3_PREFIX)
#   AGENT_S3_PREFIX s3://bucket/prefix; suffix /linux/codedeploy-agent is appended
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

# ── Configuration ────────────────────────────────────────────────────
CONCURRENCY="${CONCURRENCY:-3}"
HOOK_SLEEP_SECONDS="${HOOK_SLEEP_SECONDS:-15}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
AGENT="${REPO_ROOT}/target/release/codedeploy-agent"

REMOTE_AGENT="/tmp/codedeploy-agent"
REMOTE_E2E_DIR="/tmp/codedeploy-agent-ec2-concurrent"
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
naming_init "ec2-concurrent"

check_prereqs() {
    ensure_local_agent linux "$AGENT"
    command -v aws >/dev/null || die "AWS CLI not found"
    command -v jq >/dev/null  || die "jq not found"
    command -v zip >/dev/null || die "zip not found"
    if ! command -v session-manager-plugin >/dev/null 2>&1; then
        info "WARNING: session-manager-plugin not found — 'run' (interactive mode) will not work"
        info "  Install from: https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html"
    fi
    aws sts get-caller-identity >/dev/null || die "No valid AWS credentials in environment (aws sts get-caller-identity failed)"

    if ! [[ "$CONCURRENCY" =~ ^[0-9]+$ ]]; then
        die "CONCURRENCY must be a non-negative integer, got: $CONCURRENCY"
    fi
    if [[ "$CONCURRENCY" -lt 1 || "$CONCURRENCY" -gt 16 ]]; then
        die "CONCURRENCY must be 1..16 (the agent's MAX_CONCURRENT_COMMANDS cap), got: $CONCURRENCY"
    fi
}

save_state() { echo "$1" | jq -S '.' > "$STATE_FILE"; }
load_state() { cat "$STATE_FILE" 2>/dev/null || echo '{}'; }

# Run a command on the EC2 instance via SSM send-command.
# Prints heartbeat dots while polling so a slow round-trip doesn't look
# like a hang. Pass HEARTBEAT_EVERY=N to change the dot cadence.
ssm_cmd() {
    local instance_id="$1" region="$2"; shift 2
    local cmd_string="$*"
    local heartbeat_every="${HEARTBEAT_EVERY:-5}"

    local cmd_id
    cmd_id=$(aws ssm send-command \
        --instance-ids "$instance_id" \
        --document-name "AWS-RunShellScript" \
        --parameters "commands=[\"$cmd_string\"]" \
        --cloud-watch-output-config '{"CloudWatchOutputEnabled":true}' \
        --query 'Command.CommandId' --output text \
        --region "$region")

    local status=""
    local i
    for i in $(seq 1 60); do
        local invocation
        invocation=$(aws ssm get-command-invocation \
            --command-id "$cmd_id" \
            --instance-id "$instance_id" \
            --region "$region" 2>&1) || true

        status=$(echo "$invocation" | jq -r '.Status // empty' 2>/dev/null) || true

        case "$status" in
            Success)
                (( i > 1 )) && echo "" >&2
                echo "$invocation" | jq -r '.StandardOutputContent // empty' 2>/dev/null
                return 0
                ;;
            Failed|TimedOut|Cancelled)
                (( i > 1 )) && echo "" >&2
                local stderr_out
                stderr_out=$(echo "$invocation" | jq -r '.StandardErrorContent // empty' 2>/dev/null)
                die "SSM command failed (status=${status}): ${stderr_out}"
                ;;
            *)
                # Print a dot every Nth poll so the user can see progress.
                if (( i % heartbeat_every == 0 )); then
                    printf '.' >&2
                fi
                sleep 2
                ;;
        esac
    done
    echo "" >&2
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
    info "Setting up concurrent-deployment test (CONCURRENCY=${CONCURRENCY}) in ${REGION}..."

    # 1. IAM role + instance profile for the agent on EC2.
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
                        "ssm:UpdateInstanceInformation",
                        "ssmmessages:CreateControlChannel",
                        "ssmmessages:CreateDataChannel",
                        "ssmmessages:OpenControlChannel",
                        "ssmmessages:OpenDataChannel",
                        "ec2messages:GetMessages",
                        "ec2messages:AcknowledgeMessage",
                        "ec2messages:SendReply"
                    ],
                    "Resource": "*"
                }
            ]
        }' \
        --region "$REGION"

    info "Creating instance profile: ${INSTANCE_PROFILE_NAME}"
    if ! aws iam get-instance-profile --instance-profile-name "$INSTANCE_PROFILE_NAME" --region "$REGION" >/dev/null 2>&1; then
        aws iam create-instance-profile --instance-profile-name "$INSTANCE_PROFILE_NAME" --region "$REGION"
        aws iam add-role-to-instance-profile \
            --instance-profile-name "$INSTANCE_PROFILE_NAME" \
            --role-name "$AGENT_ROLE_NAME" \
            --region "$REGION"
    fi

    # 2. Service role for CodeDeploy.
    info "Creating service role: ${SERVICE_ROLE_NAME}"
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
        --query 'Parameter.Value' --output text --region "$REGION")
    info "  AMI: ${AMI_ID}"

    info "Waiting for instance profile to propagate..."
    sleep 10

    info "Launching EC2 instance..."
    INSTANCE_ID=$(aws ec2 run-instances \
        --image-id "$AMI_ID" \
        --instance-type t3.small \
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

    wait_ssm_ready "$INSTANCE_ID" "$REGION"

    # 4. Create S3 bucket + upload agent + revision.
    info "Creating S3 bucket: ${BUCKET_NAME}"
    if ! aws s3api head-bucket --bucket "$BUCKET_NAME" --region "$REGION" 2>/dev/null; then
        if [[ "$REGION" == "us-east-1" ]]; then
            aws s3api create-bucket --bucket "$BUCKET_NAME" --region "$REGION"
        else
            aws s3api create-bucket --bucket "$BUCKET_NAME" --region "$REGION" \
                --create-bucket-configuration "LocationConstraint=${REGION}"
        fi
    fi

    info "Uploading agent binary to S3..."
    aws s3 cp "$AGENT" "s3://${BUCKET_NAME}/agent-binary" --region "$REGION"

    info "Downloading agent binary on instance..."
    ssm_cmd "$INSTANCE_ID" "$REGION" "aws s3 cp s3://${BUCKET_NAME}/agent-binary ${REMOTE_AGENT} --region ${REGION} && chmod +x ${REMOTE_AGENT}"

    info "Setting up remote directories and config..."
    ssm_cmd "$INSTANCE_ID" "$REGION" "mkdir -p ${REMOTE_LOG_DIR} ${REMOTE_PID_DIR} ${REMOTE_ROOT_DIR} && chmod -R 777 ${REMOTE_E2E_DIR}"
    ssm_cmd "$INSTANCE_ID" "$REGION" "cat > ${REMOTE_CONFIG} << 'EOFCONFIG'
verbose: true
wait_between_runs: 3
log_dir: ${REMOTE_LOG_DIR}
pid_dir: ${REMOTE_PID_DIR}
root_dir: ${REMOTE_ROOT_DIR}
EOFCONFIG"

    # 5. Build the sample revision (hooks sleep so deployments overlap visibly).
    info "Building sample revision (AfterInstall sleeps ${HOOK_SLEEP_SECONDS}s for overlap)..."
    REVISION_DIR=$(mktemp -d)
    cat > "${REVISION_DIR}/appspec.yml" << 'APPSPEC'
version: 0.0
os: linux
hooks:
  BeforeInstall:
    - location: scripts/before_install.sh
      timeout: 60
  AfterInstall:
    - location: scripts/after_install.sh
      timeout: 120
APPSPEC

    mkdir -p "${REVISION_DIR}/scripts"
    cat > "${REVISION_DIR}/scripts/before_install.sh" << SCRIPT
#!/bin/bash
echo "[hook] BeforeInstall start  deployment_id=\${DEPLOYMENT_ID}  group_id=\${DEPLOYMENT_GROUP_ID}  ts=\$(date +%H:%M:%S.%N)"
echo "[hook] BeforeInstall end    deployment_id=\${DEPLOYMENT_ID}  group_id=\${DEPLOYMENT_GROUP_ID}  ts=\$(date +%H:%M:%S.%N)"
SCRIPT
    chmod +x "${REVISION_DIR}/scripts/before_install.sh"

    cat > "${REVISION_DIR}/scripts/after_install.sh" << SCRIPT
#!/bin/bash
echo "[hook] AfterInstall start   deployment_id=\${DEPLOYMENT_ID}  group_id=\${DEPLOYMENT_GROUP_ID}  ts=\$(date +%H:%M:%S.%N)"
sleep ${HOOK_SLEEP_SECONDS}
echo "[hook] AfterInstall end     deployment_id=\${DEPLOYMENT_ID}  group_id=\${DEPLOYMENT_GROUP_ID}  ts=\$(date +%H:%M:%S.%N)"
SCRIPT
    chmod +x "${REVISION_DIR}/scripts/after_install.sh"

    REVISION_ZIP="/tmp/${PREFIX}-revision.zip"
    (cd "$REVISION_DIR" && zip -r "$REVISION_ZIP" .)
    aws s3 cp "$REVISION_ZIP" "s3://${BUCKET_NAME}/revision.zip" --region "$REGION"
    rm -rf "$REVISION_DIR" "$REVISION_ZIP"

    # 6. Create CodeDeploy application + N deployment groups, all targeting the same instance.
    info "Creating application: ${APP_NAME}"
    if aws deploy get-application --application-name "$APP_NAME" --region "$REGION" >/dev/null 2>&1; then
        info "  Application already exists, reusing"
    else
        aws deploy create-application \
            --application-name "$APP_NAME" \
            --compute-platform Server \
            --region "$REGION"
    fi

    info "Waiting for IAM role to propagate before creating deployment groups..."
    sleep 10

    declare -a deployment_groups=()
    for i in $(seq 1 "$CONCURRENCY"); do
        local dg="${DG_BASE}-${i}"
        deployment_groups+=("$dg")
        info "Creating deployment group: ${dg}"
        if aws deploy get-deployment-group --application-name "$APP_NAME" --deployment-group-name "$dg" --region "$REGION" >/dev/null 2>&1; then
            info "  Deployment group already exists, updating"
            aws deploy update-deployment-group \
                --application-name "$APP_NAME" \
                --current-deployment-group-name "$dg" \
                --ec2-tag-filters "Key=${TAG_KEY},Value=${TAG_VALUE},Type=KEY_AND_VALUE" \
                --service-role-arn "$SERVICE_ROLE_ARN" \
                --region "$REGION"
        else
            aws deploy create-deployment-group \
                --application-name "$APP_NAME" \
                --deployment-group-name "$dg" \
                --ec2-tag-filters "Key=${TAG_KEY},Value=${TAG_VALUE},Type=KEY_AND_VALUE" \
                --service-role-arn "$SERVICE_ROLE_ARN" \
                --deployment-config-name "CodeDeployDefault.AllAtOnce" \
                --region "$REGION"
        fi
    done

    info "Waiting for IAM role propagation..."
    sleep 15

    # 7. Save state.
    local groups_json
    groups_json=$(printf '%s\n' "${deployment_groups[@]}" | jq -R . | jq -s .)
    save_state "$(jq -n \
        --arg region "$REGION" \
        --arg app "$APP_NAME" \
        --argjson groups "$groups_json" \
        --argjson concurrency "$CONCURRENCY" \
        --arg instance "$INSTANCE_ID" \
        --arg agent_role "$AGENT_ROLE_NAME" \
        --arg profile "$INSTANCE_PROFILE_NAME" \
        --arg svc_role "$SERVICE_ROLE_NAME" \
        --arg svc_arn "$SERVICE_ROLE_ARN" \
        --arg bucket "$BUCKET_NAME" \
        '{region: $region, app_name: $app, deployment_groups: $groups, concurrency: $concurrency,
          instance_id: $instance, agent_role_name: $agent_role, instance_profile_name: $profile,
          service_role_name: $svc_role, service_role_arn: $svc_arn, bucket_name: $bucket}')"

    info "Setup complete. State saved to ${STATE_FILE}"
    info ""
    info "Next steps:"
    info "  1. ./scripts/e2e-ec2-concurrent.sh run     # Start the agent on EC2 in foreground (SSM Session Manager)"
    info "  2. ./scripts/e2e-ec2-concurrent.sh deploy  # trigger ${CONCURRENCY} parallel deployments"
    info "  3. ./scripts/e2e-ec2-concurrent.sh status  # wait until all complete"
    info "  4. ./scripts/e2e-ec2-concurrent.sh logs    # show overlap evidence"
}

# ── Run agent (foreground, via SSM Session Manager) ─────────────────
# Opens an interactive SSM session and runs the agent in the foreground
# of your terminal. Requires session-manager-plugin. Press Ctrl-C to
# stop the agent.
cmd_run() {
    STATE=$(load_state)
    INSTANCE_ID=$(echo "$STATE" | jq -r '.instance_id')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')
    [[ "$INSTANCE_ID" != "null" ]] || die "No state found — run 'setup' first"

    if ! command -v session-manager-plugin >/dev/null 2>&1; then
        printf 'ERROR: session-manager-plugin not installed — `run` is interactive and requires it.\n  Install: https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html\n  Workaround: ./scripts/e2e-ec2-concurrent.sh all (runs the full cycle in the background, no plugin needed).\n' >&2
        exit 1
    fi

    info "Starting agent on ${INSTANCE_ID} via SSM (Ctrl-C to stop)..."
    info "Logs: use another SSM session or '$0 logs' to tail ${REMOTE_LOG_DIR}/codedeploy-agent*"
    aws ssm start-session --target "$INSTANCE_ID" --region "$REGION_STATE" \
        --document-name AWS-StartInteractiveCommand \
        --parameters command="CODEDEPLOY_DEVELOPER_MODE=true AWS_REGION=${REGION_STATE} ${REMOTE_AGENT} --config-file ${REMOTE_CONFIG} worker"
}

# ── Deploy (parallel) ────────────────────────────────────────────────
cmd_deploy() {
    check_prereqs
    STATE=$(load_state)
    APP=$(echo "$STATE" | jq -r '.app_name')
    BUCKET=$(echo "$STATE" | jq -r '.bucket_name')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')

    # Use a while-read loop (avoids mapfile exit-code quirks under pipefail).
    local groups_text
    groups_text=$(echo "$STATE" | jq -r '.deployment_groups[]')
    DG_LIST=()
    while IFS= read -r line; do
        [[ -n "$line" ]] && DG_LIST+=("$line")
    done <<< "$groups_text"

    [[ "$APP" != "null" ]] || die "No state found — run 'setup' first"
    [[ "${#DG_LIST[@]}" -gt 0 ]] || die "No deployment groups recorded in state"

    info "Triggering ${#DG_LIST[@]} parallel deployments..."

    declare -a id_files=()
    declare -a err_files=()
    declare -a pids=()
    for dg in "${DG_LIST[@]}"; do
        local id_file err_file
        id_file=$(mktemp)
        err_file=$(mktemp)
        id_files+=("$id_file")
        err_files+=("$err_file")
        (
            AWS_PAGER='' aws deploy create-deployment \
                --application-name "$APP" \
                --deployment-group-name "$dg" \
                --revision "revisionType=S3,s3Location={bucket=${BUCKET},key=revision.zip,bundleType=zip}" \
                --region "$REGION_STATE" \
                --query 'deploymentId' --output text > "$id_file" 2> "$err_file"
        ) &
        pids+=($!)
    done

    local fail=0
    for pid in "${pids[@]}"; do
        wait "$pid" || fail=1
    done

    declare -a deployment_ids=()
    local i=0
    for f in "${id_files[@]}"; do
        local id err
        id=$(<"$f")
        err=$(<"${err_files[$i]}")
        rm -f "$f" "${err_files[$i]}"
        i=$((i + 1))
        if [[ -z "$id" || "$id" == "None" ]]; then
            die "create-deployment returned no id (one of the parallel calls failed): ${err}"
        fi
        deployment_ids+=("$id")
    done
    [[ "$fail" -eq 0 ]] || die "One or more create-deployment calls failed"

    info "Created deployments:"
    paste <(printf '%s\n' "${DG_LIST[@]}") <(printf '%s\n' "${deployment_ids[@]}") \
        | awk -F'\t' '{printf "  %-40s %s\n", $1, $2}'

    local ids_json
    ids_json=$(printf '%s\n' "${deployment_ids[@]}" | jq -R . | jq -s .)
    save_state "$(echo "$STATE" | jq --argjson ids "$ids_json" '.deployment_ids = $ids')"
}

# ── Status (wait for all) ────────────────────────────────────────────
cmd_status() {
    check_prereqs
    STATE=$(load_state)
    REGION_STATE=$(echo "$STATE" | jq -r '.region')

    # Use a while-read loop (avoids mapfile exit-code quirks under pipefail).
    local ids_text
    ids_text=$(echo "$STATE" | jq -r '.deployment_ids[]?')
    IDS=()
    while IFS= read -r line; do
        [[ -n "$line" ]] && IDS+=("$line")
    done <<< "$ids_text"
    [[ "${#IDS[@]}" -gt 0 ]] || die "No deployments recorded — run 'deploy' first"

    info "Polling ${#IDS[@]} deployments..."
    if ! wait_for_deployments "$REGION_STATE" 600 "${IDS[@]}"; then
        die "Timeout: deployments not complete after 10 min"
    fi

    info "Final status:"
    print_deployment_table "$REGION_STATE" "${IDS[@]}"

    if (( DEPLOY_SUCCEEDED == DEPLOY_TOTAL )); then
        print_banner "PASS" "Concurrent E2E — all deployments Succeeded" \
            "Total: ${DEPLOY_TOTAL}  Succeeded: ${DEPLOY_SUCCEEDED}  Failed: ${DEPLOY_FAILED}  Stopped: ${DEPLOY_STOPPED}" \
            "Elapsed: ${DEPLOY_ELAPSED}s"
        print_deployment_outcome SUCCEEDED
    else
        print_banner "FAIL" "Concurrent E2E — one or more deployments did not succeed" \
            "Total: ${DEPLOY_TOTAL}  Succeeded: ${DEPLOY_SUCCEEDED}  Failed: ${DEPLOY_FAILED}  Stopped: ${DEPLOY_STOPPED}" \
            "Elapsed: ${DEPLOY_ELAPSED}s"
        print_deployment_outcome FAILED
        die "One or more deployments did not succeed"
    fi
}

# ── Logs (proof of overlap) ──────────────────────────────────────────
cmd_logs() {
    check_prereqs
    STATE=$(load_state)
    INSTANCE_ID=$(echo "$STATE" | jq -r '.instance_id')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')
    [[ "$INSTANCE_ID" != "null" ]] || die "No state found — run 'setup' first"

    info "Fetching agent logs from ${INSTANCE_ID}..."

    info ""
    info "Agent log (codedeploy-agent.*):"
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "tail -n 200 ${REMOTE_LOG_DIR}/codedeploy-agent* 2>/dev/null | grep -E 'deployment_id|deployment|Executing|hook' | tail -100" || true

    info ""
    info "Hook output (proves overlap — look for interleaved start/end timestamps):"
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "find ${REMOTE_ROOT_DIR} -name 'scripts.log' -exec grep -h '\\[hook\\]' {} \\; 2>/dev/null | sort -k 5" || true
}

# ── Teardown ─────────────────────────────────────────────────────────
cmd_teardown() {
    info "Tearing down concurrent-deployment test resources..."
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
            --role-name "${AGENT_ROLE:-${PREFIX}-agent-role}" \
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
        info "Deleting application: ${APP} (cascades to deployment groups)"
        aws deploy delete-application \
            --application-name "$APP" \
            --region "$REGION_STATE" 2>/dev/null || true
    fi

    if [[ -n "$ROLE" ]]; then
        info "Deleting service role: ${ROLE}"
        aws iam detach-role-policy --role-name "$ROLE" --policy-arn "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole" 2>/dev/null || true
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

# ── All (full automated cycle) ───────────────────────────────────────
cmd_all() {
    trap 'info "Error detected, running teardown..."; replay_deployment_outcome; cmd_teardown' ERR
    cmd_setup

    STATE=$(load_state)
    INSTANCE_ID=$(echo "$STATE" | jq -r '.instance_id')
    REGION_STATE=$(echo "$STATE" | jq -r '.region')

    info "Starting agent in background on EC2..."
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "setsid bash -c 'CODEDEPLOY_DEVELOPER_MODE=true AWS_REGION=${REGION_STATE} ${REMOTE_AGENT} --config-file ${REMOTE_CONFIG} worker' </dev/null > ${REMOTE_E2E_DIR}/agent.out 2>&1 & disown"
    sleep 5
    ssm_cmd "$INSTANCE_ID" "$REGION_STATE" "pgrep -f '[c]odedeploy-agent'" >/dev/null || die "Agent failed to start on EC2"

    cmd_deploy
    cmd_status || true
    cmd_logs
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
        echo "Environment variables:"
        echo "  CONCURRENCY           number of parallel deployment groups (1..16, default 3)"
        echo "  HOOK_SLEEP_SECONDS    sleep duration in AfterInstall hook (default 15)"
        echo "  AWS_REGION            AWS region (default us-east-1)"
        echo ""
        echo "Commands:"
        echo "  setup     Create IAM role, launch EC2, copy agent, create app + N deployment groups"
        echo "  run       Start the agent on EC2 in foreground (SSM Session Manager)"
        echo "  deploy    Trigger N deployments in parallel"
        echo "  status    Wait for all deployments and assert all Succeeded"
        echo "  logs      Show agent logs and hook overlap evidence"
        echo "  teardown  Delete all AWS resources"
        echo "  all       Full cycle: setup → background-run → deploy → status → logs → teardown"
        ;;
esac
