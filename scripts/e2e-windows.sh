#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# Windows build + E2E test for the Rust CodeDeploy agent.
#
# Single script that handles the full lifecycle: launch a Windows Server
# 2022 EC2 instance, install the Rust/MSYS2 toolchain, build the agent,
# install it as a Windows service, trigger a real CodeDeploy deployment,
# and verify it succeeds.
#
# Connectivity uses AWS SSM Session Manager (HTTPS), so it works from
# restricted networks where port 22 / RDP are blocked.
#
# Usage:
#   ./scripts/e2e-windows.sh setup       # Launch Windows EC2 + install toolchain
#   ./scripts/e2e-windows.sh build       # Upload source + cargo build --release
#   ./scripts/e2e-windows.sh install     # Install agent as Windows service + start it
#   ./scripts/e2e-windows.sh deploy      # Trigger a CodeDeploy deployment
#   ./scripts/e2e-windows.sh status      # Check deployment status
#   ./scripts/e2e-windows.sh logs        # Show agent logs from the instance
#   ./scripts/e2e-windows.sh fetch       # Download the built .exe locally
#   ./scripts/e2e-windows.sh shell       # Interactive SSM session to the instance
#   ./scripts/e2e-windows.sh teardown    # Terminate instance + delete all AWS resources
#   ./scripts/e2e-windows.sh build-only  # setup + build + fetch + teardown (no E2E)
#   ./scripts/e2e-windows.sh test-only   # setup + build + install + deploy + verify + teardown
#   ./scripts/e2e-windows.sh all         # setup + build + install + deploy + verify + fetch + teardown
#
# Prerequisites:
#   - AWS credentials in the environment with EC2/S3/SSM/IAM/CodeDeploy access
#   - AWS CLI v2 installed
#   - jq installed
#   - session-manager-plugin installed (optional, for 'shell')
#
# Optional: skip toolchain install + cargo build by pointing at a pre-built .exe in S3.
#   AGENT_S3_URI    full s3://bucket/key (overrides AGENT_S3_PREFIX)
#   AGENT_S3_PREFIX s3://bucket/prefix; suffix /windows/codedeploy-agent.exe is appended
# ──────────────────────────────────────────────────────────────────────
set -euo pipefail

# ── Configuration ────────────────────────────────────────────────────
INSTANCE_TYPE="${CODEDEPLOY_WIN_INSTANCE_TYPE:-t3.large}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUTPUT_DIR="${REPO_ROOT}/build/windows"

# Remote paths on Windows.
REMOTE_BUILD_DIR='C:\codedeploy-build'
REMOTE_SRC_DIR='C:\codedeploy-build\src'
REMOTE_INSTALL_DIR='C:\ProgramData\Amazon\CodeDeploy'
REMOTE_AGENT="${REMOTE_INSTALL_DIR}\\codedeploy-agent.exe"
REMOTE_CONFIG="${REMOTE_INSTALL_DIR}\\codedeployagent.yml"
REMOTE_LOG_DIR="${REMOTE_INSTALL_DIR}\\log"

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
naming_init "windows"

# Per-script alias — the Windows script historically used ROLE_NAME.
ROLE_NAME="${AGENT_ROLE_NAME}"

check_prereqs() {
    command -v aws >/dev/null || die "AWS CLI not found"
    command -v jq >/dev/null || die "jq not found"
    aws sts get-caller-identity >/dev/null 2>&1 || die "No valid AWS credentials in environment"
}

save_state() { echo "$1" | jq -S '.' > "$STATE_FILE"; }
load_state() { cat "$STATE_FILE" 2>/dev/null || echo '{}'; }
get_field() { load_state | jq -r ".$1 // empty"; }

# Run a PowerShell command on the Windows instance via SSM.
# Optional third positional arg: max poll iterations (default 360 = 30 min).
ssm_ps() {
    local instance_id="$1" region="$2"; shift 2
    local max_polls=360
    if [[ "${1:-}" =~ ^[0-9]+$ ]]; then
        max_polls="$1"; shift
    fi
    local ps_command="$*"

    local params_file
    params_file=$(mktemp)
    jq -n --arg cmd "$ps_command" '{"commands":[$cmd]}' > "$params_file"

    local cmd_id
    cmd_id=$(aws ssm send-command \
        --instance-ids "$instance_id" \
        --document-name "AWS-RunPowerShellScript" \
        --parameters "file://${params_file}" \
        --timeout-seconds 3600 \
        --query 'Command.CommandId' --output text \
        --region "$region")
    rm -f "$params_file"

    local status=""
    for i in $(seq 1 "$max_polls"); do
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
                local stderr_out stdout_out
                stderr_out=$(echo "$invocation" | jq -r '.StandardErrorContent // empty' 2>/dev/null)
                stdout_out=$(echo "$invocation" | jq -r '.StandardOutputContent // empty' 2>/dev/null)
                echo "STDOUT: ${stdout_out}" >&2
                echo "STDERR: ${stderr_out}" >&2
                die "SSM command failed (status=${status})"
                ;;
            ""|InProgress|Pending|Delayed)
                if (( i % 30 == 0 )); then
                    info "  Still running... ($((i * 5))s elapsed)"
                fi
                sleep 5
                ;;
            *)
                sleep 5
                ;;
        esac
    done

    die "SSM command timed out after $((max_polls * 5))s (command_id=${cmd_id})"
}

# Best-effort SSM command — does not abort on failure.
ssm_ps_safe() {
    local instance_id="$1" region="$2"; shift 2
    local ps_command="$*"

    local params_file
    params_file=$(mktemp)
    jq -n --arg cmd "$ps_command" '{"commands":[$cmd]}' > "$params_file"

    local cmd_id
    cmd_id=$(aws ssm send-command \
        --instance-ids "$instance_id" \
        --document-name "AWS-RunPowerShellScript" \
        --parameters "file://${params_file}" \
        --timeout-seconds 30 \
        --query 'Command.CommandId' --output text \
        --region "$region" 2>/dev/null) || { rm -f "$params_file"; return 0; }
    rm -f "$params_file"

    for i in $(seq 1 15); do
        local status
        status=$(aws ssm get-command-invocation \
            --command-id "$cmd_id" \
            --instance-id "$instance_id" \
            --region "$region" \
            --query 'Status' --output text 2>/dev/null) || true
        case "$status" in
            Success|Failed|TimedOut|Cancelled) return 0 ;;
            *) sleep 2 ;;
        esac
    done
    return 0
}

wait_ssm_ready() {
    local instance_id="$1" region="$2"

    info "Waiting for SSM agent to come online (Windows boot takes 2-4 min)..."
    for i in $(seq 1 60); do
        local ping_status
        ping_status=$(aws ssm describe-instance-information \
            --filters "Key=InstanceIds,Values=${instance_id}" \
            --query 'InstanceInformationList[0].PingStatus' --output text \
            --region "$region" 2>/dev/null) || true

        if [[ "$ping_status" == "Online" ]]; then
            info "  SSM agent is online"
            return 0
        fi
        if (( i % 6 == 0 )); then
            info "  Waiting... ($(( i * 10 ))s elapsed)"
        fi
        sleep 10
    done

    die "SSM agent did not come online after 10 minutes"
}

# ── Setup ────────────────────────────────────────────────────────────
cmd_setup() {
    check_prereqs
    info "Setting up Windows build + E2E instance in ${REGION}..."

    # naming_init populated AWS_ACCOUNT_ID + BUCKET_NAME globally.

    # 1. Create IAM role + instance profile.
    info "Creating IAM role: ${ROLE_NAME}"
    if aws iam get-role --role-name "$ROLE_NAME" >/dev/null 2>&1; then
        info "  IAM role already exists, reusing"
    else
        aws iam create-role \
            --role-name "$ROLE_NAME" \
            --assume-role-policy-document '{
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "ec2.amazonaws.com"},
                    "Action": "sts:AssumeRole"
                }]
            }'
    fi

    aws iam put-role-policy \
        --role-name "$ROLE_NAME" \
        --policy-name "${AGENT_ROLE_POLICY_NAME}" \
        --policy-document "{
            \"Version\": \"2012-10-17\",
            \"Statement\": [
                {
                    \"Effect\": \"Allow\",
                    \"Action\": [
                        \"codedeploy-commands:*\",
                        \"s3:GetObject\",
                        \"s3:GetObjectVersion\",
                        \"s3:PutObject\",
                        \"s3:ListBucket\"
                    ],
                    \"Resource\": \"*\"
                },
                {
                    \"Effect\": \"Allow\",
                    \"Action\": [
                        \"ssm:UpdateInstanceInformation\",
                        \"ssmmessages:CreateControlChannel\",
                        \"ssmmessages:CreateDataChannel\",
                        \"ssmmessages:OpenControlChannel\",
                        \"ssmmessages:OpenDataChannel\",
                        \"ec2messages:AcknowledgeMessage\",
                        \"ec2messages:DeleteMessage\",
                        \"ec2messages:FailMessage\",
                        \"ec2messages:GetEndpoint\",
                        \"ec2messages:GetMessages\",
                        \"ec2messages:SendReply\"
                    ],
                    \"Resource\": \"*\"
                }
            ]
        }"

    info "Creating instance profile: ${INSTANCE_PROFILE_NAME}"
    if aws iam get-instance-profile --instance-profile-name "$INSTANCE_PROFILE_NAME" >/dev/null 2>&1; then
        info "  Instance profile already exists, reusing"
    else
        aws iam create-instance-profile \
            --instance-profile-name "$INSTANCE_PROFILE_NAME"
        aws iam add-role-to-instance-profile \
            --instance-profile-name "$INSTANCE_PROFILE_NAME" \
            --role-name "$ROLE_NAME" 2>/dev/null || true
    fi

    # 2. Create CodeDeploy service role.
    info "Creating CodeDeploy service role: ${SERVICE_ROLE_NAME}"
    if aws iam get-role --role-name "$SERVICE_ROLE_NAME" >/dev/null 2>&1; then
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
            }'
    fi
    aws iam attach-role-policy \
        --role-name "$SERVICE_ROLE_NAME" \
        --policy-arn "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole"

    local service_role_arn
    service_role_arn=$(aws iam get-role --role-name "$SERVICE_ROLE_NAME" --query 'Role.Arn' --output text)

    # 3. Create S3 bucket.
    info "Creating S3 bucket: ${BUCKET_NAME}"
    if aws s3api head-bucket --bucket "$BUCKET_NAME" --region "$REGION" 2>/dev/null; then
        info "  Bucket already exists, reusing"
    elif [[ "$REGION" == "us-east-1" ]]; then
        aws s3api create-bucket --bucket "$BUCKET_NAME" --region "$REGION"
    else
        aws s3api create-bucket --bucket "$BUCKET_NAME" --region "$REGION" \
            --create-bucket-configuration "LocationConstraint=${REGION}"
    fi

    # 4. Launch Windows Server 2022 instance.
    info "Looking up latest Windows Server 2022 AMI..."
    local ami_id
    ami_id=$(aws ssm get-parameter \
        --name "/aws/service/ami-windows-latest/Windows_Server-2022-English-Full-Base" \
        --query 'Parameter.Value' --output text \
        --region "$REGION")
    info "  AMI: ${ami_id}"

    info "Waiting for instance profile to propagate..."
    sleep 10

    info "Launching EC2 instance (${INSTANCE_TYPE})..."
    local instance_id
    instance_id=$(aws ec2 run-instances \
        --image-id "$ami_id" \
        --instance-type "$INSTANCE_TYPE" \
        --iam-instance-profile "Name=${INSTANCE_PROFILE_NAME}" \
        --metadata-options "HttpTokens=required,HttpPutResponseHopLimit=2,HttpEndpoint=enabled" \
        --block-device-mappings '[{"DeviceName":"/dev/sda1","Ebs":{"VolumeSize":50,"VolumeType":"gp3"}}]' \
        --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=${PREFIX}},{Key=${TAG_KEY},Value=${TAG_VALUE}}]" \
        --query 'Instances[0].InstanceId' --output text \
        --region "$REGION")
    info "  Instance: ${instance_id}"

    info "Waiting for instance to be running..."
    aws ec2 wait instance-running --instance-ids "$instance_id" --region "$REGION"

    info "Waiting for status checks (Windows boot takes 2-4 min)..."
    aws ec2 wait instance-status-ok --instance-ids "$instance_id" --region "$REGION"

    wait_ssm_ready "$instance_id" "$REGION"

    # 5. Install build toolchain (Chocolatey + MSYS2 + Rust) — skipped
    # when a pre-built binary will be used (AGENT_S3_URI / AGENT_S3_PREFIX).
    if using_prebuilt_agent windows; then
        info "Skipping toolchain install — pre-built agent will be fetched from $(agent_s3_uri_for windows)"
    else
        info "Installing Chocolatey..."
        ssm_ps "$instance_id" "$REGION" '
            $ErrorActionPreference = "Stop"
            Set-ExecutionPolicy Bypass -Scope Process -Force
            [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
            iex ((New-Object System.Net.WebClient).DownloadString("https://community.chocolatey.org/install.ps1"))
            choco --version
        '

        info "Installing MSYS2 (provides MinGW GCC + OpenSSL)..."
        ssm_ps "$instance_id" "$REGION" '
            $ErrorActionPreference = "Stop"
            & C:\ProgramData\chocolatey\bin\choco.exe install msys2 -y --no-progress
            & C:\tools\msys64\usr\bin\bash.exe -lc "pacman -S --noconfirm mingw-w64-x86_64-gcc mingw-w64-x86_64-openssl mingw-w64-x86_64-pkg-config"
            Write-Output "MSYS2 + MinGW GCC + OpenSSL installed"
        '

        info "Installing Rust toolchain (stable-x86_64-pc-windows-gnu)..."
        ssm_ps "$instance_id" "$REGION" '
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile "$env:TEMP\rustup-init.exe" -UseBasicParsing
            & "$env:TEMP\rustup-init.exe" -y --default-host x86_64-pc-windows-gnu --default-toolchain stable --profile minimal 2>$null
            $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
            rustc --version
            cargo --version
            if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Write-Error "Rust installation failed"; exit 1 }
        '
    fi

    # 6. Create CodeDeploy application + deployment group.
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
            --service-role-arn "$service_role_arn" \
            --region "$REGION"
    else
        info "Waiting for IAM role to propagate..."
        sleep 10
        aws deploy create-deployment-group \
            --application-name "$APP_NAME" \
            --deployment-group-name "$DG_NAME" \
            --ec2-tag-filters "Key=${TAG_KEY},Value=${TAG_VALUE},Type=KEY_AND_VALUE" \
            --service-role-arn "$service_role_arn" \
            --region "$REGION"
    fi

    # 7. Upload sample revision.
    info "Creating Windows revision..."
    local revision_dir
    revision_dir=$(mktemp -d)
    cat > "${revision_dir}/appspec.yml" << 'APPSPEC'
version: 0.0
os: windows
hooks:
  BeforeInstall:
    - location: scripts/before_install.ps1
      timeout: 30
  AfterInstall:
    - location: scripts/after_install.ps1
      timeout: 30
APPSPEC

    mkdir -p "${revision_dir}/scripts"
    cat > "${revision_dir}/scripts/before_install.ps1" << 'SCRIPT'
Write-Output "BeforeInstall hook running at $(Get-Date)"
Write-Output "DEPLOYMENT_ID=$env:DEPLOYMENT_ID"
Write-Output "LIFECYCLE_EVENT=$env:LIFECYCLE_EVENT"
SCRIPT

    cat > "${revision_dir}/scripts/after_install.ps1" << 'SCRIPT'
Write-Output "AfterInstall hook running at $(Get-Date)"
Write-Output "DEPLOYMENT_ID=$env:DEPLOYMENT_ID"
Write-Output "LIFECYCLE_EVENT=$env:LIFECYCLE_EVENT"
Write-Output "Deployment successful!"
SCRIPT

    local revision_zip="/tmp/${PREFIX}-revision.zip"
    (cd "$revision_dir" && zip -r "$revision_zip" .)
    aws s3 cp "$revision_zip" "s3://${BUCKET_NAME}/revision.zip" --region "$REGION"
    rm -rf "$revision_dir" "$revision_zip"

    # 8. Save state.
    save_state "$(jq -n \
        --arg region "$REGION" \
        --arg app_name "$APP_NAME" \
        --arg dg_name "$DG_NAME" \
        --arg instance_id "$instance_id" \
        --arg role_name "$ROLE_NAME" \
        --arg instance_profile_name "$INSTANCE_PROFILE_NAME" \
        --arg service_role_name "$SERVICE_ROLE_NAME" \
        --arg service_role_arn "$service_role_arn" \
        --arg bucket_name "$BUCKET_NAME" \
        --arg ami_id "$ami_id" \
        '{region: $region, app_name: $app_name, dg_name: $dg_name, instance_id: $instance_id, role_name: $role_name, instance_profile_name: $instance_profile_name, service_role_name: $service_role_name, service_role_arn: $service_role_arn, bucket_name: $bucket_name, ami_id: $ami_id}')"

    info "Setup complete!"
    info "  Instance: ${instance_id}"
    info "  Bucket:   ${BUCKET_NAME}"
    info "  State:    ${STATE_FILE}"
}

# ── Build ────────────────────────────────────────────────────────────
cmd_build() {
    check_prereqs
    local instance_id bucket region
    instance_id=$(get_field instance_id)
    bucket=$(get_field bucket_name)
    region=$(get_field region)
    [[ -n "$instance_id" ]] || die "No instance found — run 'setup' first"

    if using_prebuilt_agent windows; then
        info "Skipping build — pre-built agent will be fetched from $(agent_s3_uri_for windows)"
        return 0
    fi

    info "Packaging source for upload..."
    local src_archive="/tmp/${PREFIX}-source.tar.gz"
    tar -czf "$src_archive" \
        --transform 's,^\.,src,' \
        --exclude='./build' \
        --exclude='./.git' \
        --exclude='./target' \
        --exclude='./security-review' \
        -C "$REPO_ROOT" .
    info "  Source archive: $(du -h "$src_archive" | cut -f1)"

    info "Uploading source to S3..."
    aws s3 cp "$src_archive" "s3://${bucket}/source.tar.gz" --region "$region"
    rm -f "$src_archive"

    info "Downloading and extracting source on Windows..."
    ssm_ps "$instance_id" "$region" "
        \$ErrorActionPreference = 'Stop'
        if (Test-Path '${REMOTE_SRC_DIR}') { Remove-Item -Recurse -Force '${REMOTE_SRC_DIR}' }
        New-Item -ItemType Directory -Force -Path '${REMOTE_BUILD_DIR}' | Out-Null
        Read-S3Object -BucketName '${bucket}' -Key 'source.tar.gz' -File '${REMOTE_BUILD_DIR}\\source.tar.gz' -Region '${region}'
        cd '${REMOTE_BUILD_DIR}'
        tar -xzf source.tar.gz
        Remove-Item -Force '${REMOTE_BUILD_DIR}\\source.tar.gz'
        Remove-Item -Force '${REMOTE_SRC_DIR}\\rust-toolchain.toml' -ErrorAction SilentlyContinue
        Remove-Item -Force '${REMOTE_SRC_DIR}\\rust-toolchain' -ErrorAction SilentlyContinue
        Write-Output 'Source extracted'
        Get-ChildItem '${REMOTE_SRC_DIR}' | Select-Object Name
    "

    info "Building agent (cargo build --release)... this takes 15-20 minutes on first build"
    ssm_ps "$instance_id" "$region" 720 "
        \$env:PATH = 'C:\\tools\\msys64\\mingw64\\bin;' + \"\$env:USERPROFILE\\.cargo\\bin;\" + 'C:\\Windows\\system32;C:\\Windows;C:\\Windows\\System32\\WindowsPowerShell\\v1.0'
        \$env:OPENSSL_DIR = 'C:\\tools\\msys64\\mingw64'
        \$env:OPENSSL_STATIC = '1'
        \$env:OPENSSL_LIB_DIR = 'C:\\tools\\msys64\\mingw64\\lib'
        \$env:OPENSSL_INCLUDE_DIR = 'C:\\tools\\msys64\\mingw64\\include'
        cd '${REMOTE_SRC_DIR}'
        \$buildLog = '${REMOTE_BUILD_DIR}\\build.log'
        cargo build --release > \$buildLog 2>&1
        if (\$LASTEXITCODE -ne 0) {
            Write-Output '--- BUILD FAILED (last 20 lines) ---'
            Get-Content \$buildLog -Tail 20
            exit 1
        }
        \$exe = '${REMOTE_SRC_DIR}\\target\\release\\codedeploy-agent.exe'
        if (Test-Path \$exe) {
            \$size = (Get-Item \$exe).Length / 1MB
            Write-Output \"Build successful! Binary size: \$([math]::Round(\$size, 2)) MB\"
        } else {
            Write-Error 'Build failed — binary not found'
            exit 1
        }
    "

    info "Build complete!"
}

# ── Install (install agent as Windows service + start) ───────────────
cmd_install() {
    local instance_id bucket region
    instance_id=$(get_field instance_id)
    bucket=$(get_field bucket_name)
    region=$(get_field region)
    [[ -n "$instance_id" ]] || die "No instance found — run 'setup' first"

    local stage_ps
    if using_prebuilt_agent windows; then
        local prebuilt_uri prebuilt_bucket prebuilt_key
        prebuilt_uri=$(agent_s3_uri_for windows)
        prebuilt_bucket="${prebuilt_uri#s3://}"
        prebuilt_key="${prebuilt_bucket#*/}"
        prebuilt_bucket="${prebuilt_bucket%%/*}"
        info "Installing agent from pre-built ${prebuilt_uri}..."
        stage_ps="Read-S3Object -BucketName '${prebuilt_bucket}' -Key '${prebuilt_key}' -File '${REMOTE_AGENT}' -Region '${region}'"
    else
        info "Installing agent from local build (no S3 round-trip)..."
        stage_ps="\$src = '${REMOTE_SRC_DIR}\\target\\release\\codedeploy-agent.exe'
        if (-not (Test-Path \$src)) { Write-Error \"No build output at \$src — run 'build' first\"; exit 1 }
        Copy-Item -Path \$src -Destination '${REMOTE_AGENT}' -Force"
    fi

    ssm_ps "$instance_id" "$region" "
        \$ErrorActionPreference = 'Stop'
        New-Item -ItemType Directory -Force -Path '${REMOTE_INSTALL_DIR}' | Out-Null
        ${stage_ps}
        \$config = \"verbose: true\`nwait_between_runs: 5\"
        [System.IO.File]::WriteAllText('${REMOTE_CONFIG}', \$config)
        & '${REMOTE_AGENT}' install-service --config-file '${REMOTE_CONFIG}'
    "

    info "Starting agent service..."
    ssm_ps "$instance_id" "$region" '
        sc.exe start codedeployagent
        Start-Sleep 3
        $svc = Get-Service -Name codedeployagent
        if ($svc.Status -ne "Running") { Write-Error "Service not running: $($svc.Status)"; exit 1 }
        Write-Output "Agent service is RUNNING"
    '
}

# ── Deploy ───────────────────────────────────────────────────────────
cmd_deploy() {
    check_prereqs
    local bucket region
    bucket=$(get_field bucket_name)
    region=$(get_field region)
    [[ -n "$bucket" ]] || die "No state found — run 'setup' first"

    info "Creating deployment for ${APP_NAME} / ${DG_NAME}..."

    local deployment_id=""
    for attempt in $(seq 1 12); do
        local deploy_output
        deploy_output=$(AWS_PAGER='' aws deploy create-deployment \
            --application-name "$APP_NAME" \
            --deployment-group-name "$DG_NAME" \
            --revision "revisionType=S3,s3Location={bucket=${bucket},key=revision.zip,bundleType=zip}" \
            --region "$region" 2>&1) || true

        if echo "$deploy_output" | grep -q "IAM_ROLE_PERMISSIONS"; then
            info "  Role not yet assumable, retrying... (${attempt}/12)"
            sleep 10
            continue
        fi

        deployment_id=$(echo "$deploy_output" | jq -r '.deploymentId // empty' 2>/dev/null)
        [[ -n "$deployment_id" ]] || die "create-deployment failed: ${deploy_output}"

        sleep 3
        local dep_error
        dep_error=$(aws deploy get-deployment --deployment-id "$deployment_id" \
            --query 'deploymentInfo.errorInformation.code' --output text \
            --region "$region" 2>/dev/null) || true
        if [[ "$dep_error" == "IAM_ROLE_PERMISSIONS" ]]; then
            info "  Role not yet assumable (async), retrying... (${attempt}/12)"
            deployment_id=""
            sleep 10
            continue
        fi

        break
    done

    [[ -n "$deployment_id" ]] || die "Gave up waiting for IAM role propagation after 12 attempts"
    info "Deployment created: ${deployment_id}"

    local state
    state=$(load_state)
    echo "$state" | jq --arg id "$deployment_id" '.last_deployment_id = $id' > "$STATE_FILE"
}

# ── Status ───────────────────────────────────────────────────────────
cmd_status() {
    local deployment_id region
    deployment_id=$(get_field last_deployment_id)
    region=$(get_field region)
    [[ -n "$deployment_id" ]] || die "No deployment found — run 'deploy' first"

    info "Polling deployment ${deployment_id}..."
    if ! wait_for_deployments "$region" 600 "$deployment_id"; then
        print_banner "TIMEOUT" "Windows E2E — deployment did not reach terminal state in 10 min"
        die "Timeout"
    fi

    info "Final status:"
    print_deployment_table "$region" "$deployment_id"

    if (( DEPLOY_SUCCEEDED == DEPLOY_TOTAL )); then
        print_banner "PASS" "Windows E2E — deployment Succeeded" \
            "Deployment: ${deployment_id}" \
            "Elapsed: ${DEPLOY_ELAPSED}s"
        print_deployment_outcome SUCCEEDED
    else
        print_banner "FAIL" "Windows E2E — deployment did not succeed" \
            "Deployment: ${deployment_id}" \
            "Succeeded: ${DEPLOY_SUCCEEDED}  Failed: ${DEPLOY_FAILED}  Stopped: ${DEPLOY_STOPPED}" \
            "Elapsed: ${DEPLOY_ELAPSED}s"
        print_deployment_outcome FAILED
        die "Deployment did not succeed"
    fi
}

# ── Logs ─────────────────────────────────────────────────────────────
cmd_logs() {
    local instance_id region
    instance_id=$(get_field instance_id)
    region=$(get_field region)
    [[ -n "$instance_id" ]] || die "No state found — run 'setup' first"

    info "Agent logs from ${instance_id}:"
    ssm_ps "$instance_id" "$region" "Get-Content '${REMOTE_LOG_DIR}\\codedeploy-agent*' -ErrorAction SilentlyContinue"
}

# ── Fetch ────────────────────────────────────────────────────────────
cmd_fetch() {
    check_prereqs
    local instance_id bucket region
    instance_id=$(get_field instance_id)
    bucket=$(get_field bucket_name)
    region=$(get_field region)

    mkdir -p "$OUTPUT_DIR"
    local output_path="${OUTPUT_DIR}/codedeploy-agent.exe"

    if using_prebuilt_agent windows; then
        local prebuilt_uri
        prebuilt_uri=$(agent_s3_uri_for windows)
        info "Downloading pre-built Windows binary from ${prebuilt_uri}..."
        aws s3 cp "$prebuilt_uri" "$output_path"
    else
        [[ -n "$bucket" && -n "$instance_id" ]] || die "No state found — run 'setup' first"
        info "Uploading freshly built binary from instance to ${bucket} ..."
        ssm_ps "$instance_id" "$region" "
            \$ErrorActionPreference = 'Stop'
            \$src = '${REMOTE_SRC_DIR}\\target\\release\\codedeploy-agent.exe'
            if (-not (Test-Path \$src)) { Write-Error \"No build output at \$src — run 'build' first\"; exit 1 }
            Write-S3Object -BucketName '${bucket}' -Key 'codedeploy-agent.exe' -File \$src -Region '${region}'
        "
        info "Downloading Windows binary from S3..."
        aws s3 cp "s3://${bucket}/codedeploy-agent.exe" "$output_path" --region "$region"
    fi
    info "Downloaded: ${output_path} ($(du -h "$output_path" | cut -f1))"
}

# ── Shell ────────────────────────────────────────────────────────────
cmd_shell() {
    local instance_id region
    instance_id=$(get_field instance_id)
    region=$(get_field region)
    [[ -n "$instance_id" ]] || die "No state found — run 'setup' first"
    command -v session-manager-plugin >/dev/null || die "session-manager-plugin not installed"

    info "Starting interactive session to ${instance_id}..."
    aws ssm start-session --target "$instance_id" --region "$region"
}

# ── Teardown ─────────────────────────────────────────────────────────
cmd_teardown() {
    info "Tearing down Windows E2E resources..."
    local instance_id region role_name profile service_role app bucket
    instance_id=$(get_field instance_id)
    region=$(get_field region); region="${region:-$REGION}"
    role_name=$(get_field role_name)
    profile=$(get_field instance_profile_name)
    service_role=$(get_field service_role_name)
    app=$(get_field app_name)
    bucket=$(get_field bucket_name)

    if [[ -n "$instance_id" ]]; then
        info "Stopping agent service..."
        ssm_ps_safe "$instance_id" "$region" 'sc.exe stop codedeployagent 2>$null'

        info "Terminating instance: ${instance_id}"
        aws ec2 terminate-instances --instance-ids "$instance_id" --region "$region" >/dev/null 2>&1 || true
        info "  Waiting for termination..."
        aws ec2 wait instance-terminated --instance-ids "$instance_id" --region "$region" 2>/dev/null || true
    fi

    if [[ -n "$profile" ]]; then
        info "Deleting instance profile: ${profile}"
        aws iam remove-role-from-instance-profile \
            --instance-profile-name "$profile" \
            --role-name "${role_name:-${PREFIX}-agent-role}" 2>/dev/null || true
        aws iam delete-instance-profile \
            --instance-profile-name "$profile" 2>/dev/null || true
    fi

    if [[ -n "$role_name" ]]; then
        info "Deleting agent role: ${role_name}"
        aws iam delete-role-policy --role-name "$role_name" --policy-name "${AGENT_ROLE_POLICY_NAME}" 2>/dev/null || true
        aws iam delete-role --role-name "$role_name" 2>/dev/null || true
    fi

    if [[ -n "$app" ]]; then
        info "Deleting application: ${app}"
        aws deploy delete-application --application-name "$app" --region "$region" 2>/dev/null || true
    fi

    if [[ -n "$service_role" ]]; then
        info "Deleting service role: ${service_role}"
        aws iam detach-role-policy --role-name "$service_role" --policy-arn "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole" 2>/dev/null || true
        aws iam delete-role --role-name "$service_role" 2>/dev/null || true
    fi

    if [[ -n "$bucket" ]]; then
        info "Deleting S3 bucket: ${bucket}"
        aws s3 rm "s3://${bucket}" --recursive --region "$region" 2>/dev/null || true
        aws s3api delete-bucket --bucket "$bucket" --region "$region" 2>/dev/null || true
    fi

    rm -f "$STATE_FILE"
    info "Teardown complete!"
}

# ── Composite commands ───────────────────────────────────────────────

# Build only — no E2E test.
cmd_build_only() {
    trap 'info "Error detected, running teardown..."; replay_deployment_outcome; cmd_teardown' ERR
    cmd_setup
    cmd_build
    cmd_fetch
    cmd_teardown
    info "Build-only complete! Binary at: ${OUTPUT_DIR}/codedeploy-agent.exe"
}

# Test only — build + deploy via CodeDeploy service.
cmd_test_only() {
    trap 'info "Error detected, running teardown..."; replay_deployment_outcome; cmd_teardown' ERR
    cmd_setup
    cmd_build
    cmd_install
    cmd_deploy
    cmd_status
    cmd_logs
    replay_deployment_outcome
    cmd_teardown
    info "Windows E2E test PASSED"
}

# Full cycle — build + test + fetch binary.
cmd_all() {
    trap 'info "Error detected, running teardown..."; replay_deployment_outcome; cmd_teardown' ERR
    cmd_setup
    cmd_build
    cmd_fetch
    cmd_install
    cmd_deploy
    cmd_status
    cmd_logs
    replay_deployment_outcome
    cmd_teardown
    info "All done! Windows E2E PASSED. Binary at: ${OUTPUT_DIR}/codedeploy-agent.exe"
}

# ── Main ─────────────────────────────────────────────────────────────
case "${1:-help}" in
    setup)      cmd_setup ;;
    build)      cmd_build ;;
    install)    cmd_install ;;
    deploy)     cmd_deploy ;;
    status)     cmd_status ;;
    logs)       cmd_logs ;;
    fetch)      cmd_fetch ;;
    shell)      cmd_shell ;;
    teardown)   cmd_teardown ;;
    build-only) cmd_build_only ;;
    test-only)  cmd_test_only ;;
    all)        cmd_all ;;
    help|*)
        echo "Usage: $0 {setup|build|install|deploy|status|logs|fetch|shell|teardown|build-only|test-only|all}"
        echo ""
        echo "Individual commands:"
        echo "  setup       Launch Windows EC2, install Rust/MSYS2/OpenSSL, create CodeDeploy resources"
        echo "  build       Upload source, cargo build --release on the instance"
        echo "  install     Install the built binary as a Windows service and start it"
        echo "  deploy      Trigger a CodeDeploy deployment"
        echo "  status      Check last deployment status"
        echo "  logs        Show agent logs from the instance"
        echo "  fetch       Download the built .exe to build/windows/"
        echo "  shell       Interactive SSM session to the instance"
        echo "  teardown    Terminate instance + delete all AWS resources"
        echo ""
        echo "Composite commands:"
        echo "  build-only  setup + build + fetch + teardown (just produce the .exe)"
        echo "  test-only   setup + build + install + deploy + verify + teardown (E2E test)"
        echo "  all         setup + build + fetch + install + deploy + verify + teardown (everything)"
        echo ""
        echo "Environment:"
        echo "  AWS_REGION                  AWS region (default: us-east-1)"
        echo "  CODEDEPLOY_WIN_INSTANCE_TYPE      Instance type (default: t3.large)"
        echo "  AGENT_S3_URI                full s3://bucket/key to a pre-built .exe — skips toolchain + build"
        echo "  AGENT_S3_PREFIX             s3://bucket/prefix; suffix /windows/codedeploy-agent.exe is appended"
        echo ""
        echo "Toolchain: Rust stable (x86_64-pc-windows-gnu) + MSYS2 MinGW64 GCC + OpenSSL"
        echo "(Toolchain install + build are skipped when AGENT_S3_URI / AGENT_S3_PREFIX is set.)"
        ;;
esac
