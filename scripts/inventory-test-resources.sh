#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# inventory-test-resources.sh — audit and clean up E2E-test leftovers.
#
# Three modes:
#
#   list       (default)  Read-only inventory. Prints every matching
#                         resource grouped by service. Never modifies
#                         anything.
#
#   --dry-run             Same scan as list, but renders the deletion
#                         plan in dependency order so you can preview
#                         exactly what `--delete` would do.
#
#   --delete              Actually delete every matched resource in
#                         the right order. Prompts for confirmation
#                         once unless `--yes` is passed.
#
# Matches the `codedeploy-agent-*` naming used by the E2E scripts.
# Region defaults: us-east-1, us-west-2 (the two regions the
# E2E scripts default to). IAM is global so it's scanned once.
#
# Usage:
#   ./scripts/inventory-test-resources.sh
#   ./scripts/inventory-test-resources.sh --regions us-east-1,us-west-2,eu-west-1
#   ./scripts/inventory-test-resources.sh --all-regions
#   ./scripts/inventory-test-resources.sh --dry-run
#   ./scripts/inventory-test-resources.sh --delete --yes
#
# Safety:
#   - `list` and `--dry-run` are read-only.
#   - `--delete` without `--yes` prompts for explicit confirmation.
#   - Empty S3 buckets are deleted; non-empty buckets are emptied first.
#   - EC2 instances are terminated and waited on before the IAM profile
#     they reference is deleted.
# ──────────────────────────────────────────────────────────────────────
set -Eeuo pipefail

PREFIXES=("codedeploy-agent-")
DEFAULT_REGIONS=("us-east-1" "us-west-2")

# ── Helpers ──────────────────────────────────────────────────────────
die()  { echo "ERROR: $*" >&2; exit 1; }
info() { echo "==> $*"; }
warn() { echo "WARN: $*" >&2; }

# Build a JMESPath OR-chain matching any configured prefix against a
# named field. Output: starts_with(F,`p1`) || starts_with(F,`p2`)
prefix_filter() {
    local field="$1" out="" i=0
    for p in "${PREFIXES[@]}"; do
        (( i > 0 )) && out+=" || "
        out+="starts_with(${field},\`${p}\`)"
        i=$((i + 1))
    done
    printf '%s' "$out"
}

# `tr` is byte-oriented and mangles the multi-byte `─`; build the rule
# from a fixed string of box-drawing chars instead.
rule()    { printf '%s\n' '────────────────────────────────────────────────────────────────'; }
header()  { echo; rule; printf '  %s\n' "$1"; rule; }
has_rows() { [[ -n "$(echo "$1" | tr -d '[:space:]')" ]]; }

# ── Args ─────────────────────────────────────────────────────────────
MODE="list"            # list | dry-run | delete
REGIONS_ARG=""
ALL_REGIONS=0
ASSUME_YES=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        -r|--region|--regions)
            REGIONS_ARG="$2"; shift 2 ;;
        --all-regions)
            ALL_REGIONS=1; shift ;;
        --dry-run)
            MODE="dry-run"; shift ;;
        --delete)
            MODE="delete"; shift ;;
        -y|--yes)
            ASSUME_YES=1; shift ;;
        -h|--help)
            sed -n '2,/^# ─*$/p' "$0" | sed 's|^# \?||'
            exit 0 ;;
        *)
            die "Unknown argument: $1 (try --help)" ;;
    esac
done

command -v aws >/dev/null || die "aws CLI not found"
command -v jq >/dev/null  || die "jq not found"
aws sts get-caller-identity >/dev/null || die "no AWS credentials"

if (( ALL_REGIONS )); then
    info "Discovering enabled regions..."
    mapfile -t REGIONS < <(aws ec2 describe-regions --query 'Regions[].RegionName' --output text | tr '\t' '\n')
elif [[ -n "$REGIONS_ARG" ]]; then
    IFS=',' read -ra REGIONS <<< "$REGIONS_ARG"
else
    REGIONS=("${DEFAULT_REGIONS[@]}")
fi

ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)

# ── Discovery (always read-only; populates arrays) ───────────────────
declare -a INSTANCES_FLAT=()      # "region|instance-id|name-tag|state"
declare -a ROLES=()
declare -a USERS=()
declare -a PROFILES=()
declare -a BUCKETS=()
declare -a APPS_FLAT=()           # "region|app-name"
declare -a DGS_FLAT=()            # "region|app-name|dg-name"
declare -a ONPREM_FLAT=()         # "region|instance-name"

discover_iam() {
    info "Scanning IAM (global)..."
    mapfile -t ROLES < <(aws iam list-roles \
        --query "Roles[?$(prefix_filter RoleName)].RoleName" --output text | tr '\t' '\n' | sed '/^$/d')
    mapfile -t USERS < <(aws iam list-users \
        --query "Users[?$(prefix_filter UserName)].UserName" --output text | tr '\t' '\n' | sed '/^$/d')
    mapfile -t PROFILES < <(aws iam list-instance-profiles \
        --query "InstanceProfiles[?$(prefix_filter InstanceProfileName)].InstanceProfileName" --output text | tr '\t' '\n' | sed '/^$/d')
}

discover_s3() {
    info "Scanning S3 buckets (global)..."
    mapfile -t BUCKETS < <(aws s3api list-buckets \
        --query "Buckets[?$(prefix_filter Name)].Name" --output text | tr '\t' '\n' | sed '/^$/d')
}

discover_region() {
    local region="$1"
    info "Scanning ${region}..."

    # EC2 — match by tag-key codedeploy-agent-test, plus Name-tag prefix.
    local ids ids2 line
    ids=$(aws ec2 describe-instances --region "$region" \
        --filters Name=tag-key,Values=codedeploy-agent-test \
        --query 'Reservations[].Instances[?State.Name!=`terminated`].[InstanceId,Tags[?Key==`Name`].Value|[0],State.Name]' \
        --output text 2>/dev/null | sed '/^$/d')
    for p in "${PREFIXES[@]}"; do
        ids2=$(aws ec2 describe-instances --region "$region" \
            --filters "Name=tag:Name,Values=${p}*" \
            --query 'Reservations[].Instances[?State.Name!=`terminated`].[InstanceId,Tags[?Key==`Name`].Value|[0],State.Name]' \
            --output text 2>/dev/null | sed '/^$/d')
        ids=$(printf '%s\n%s\n' "$ids" "$ids2")
    done
    while IFS=$'\t' read -r iid name state; do
        [[ -n "$iid" ]] || continue
        line="${region}|${iid}|${name:-<no-name>}|${state}"
        # de-dup
        local seen=0
        for x in "${INSTANCES_FLAT[@]:-}"; do [[ "$x" == "$line" ]] && { seen=1; break; }; done
        (( seen == 0 )) && INSTANCES_FLAT+=("$line")
    done <<< "$(echo "$ids" | sort -u)"

    # CodeDeploy apps + deployment groups.
    local apps app dgs dg
    apps=$(aws deploy list-applications --region "$region" \
        --query "applications[?$(prefix_filter '@')]" --output text 2>/dev/null | tr '\t' '\n' | sed '/^$/d')
    while IFS= read -r app; do
        [[ -n "$app" ]] || continue
        APPS_FLAT+=("${region}|${app}")
        dgs=$(aws deploy list-deployment-groups --region "$region" \
            --application-name "$app" --query 'deploymentGroups[]' --output text 2>/dev/null \
            | tr '\t' '\n' | sed '/^$/d')
        while IFS= read -r dg; do
            [[ -n "$dg" ]] && DGS_FLAT+=("${region}|${app}|${dg}")
        done <<< "$dgs"
    done <<< "$apps"

    # On-prem registrations.
    local onprem
    onprem=$(aws deploy list-on-premises-instances --region "$region" \
        --query "instanceNames[?$(prefix_filter '@')]" --output text 2>/dev/null | tr '\t' '\n' | sed '/^$/d')
    while IFS= read -r name; do
        [[ -n "$name" ]] && ONPREM_FLAT+=("${region}|${name}")
    done <<< "$onprem"

    # A trailing `read` returns non-zero at EOF; don't let that fail the
    # function under `set -e`.
    return 0
}

discover_iam
discover_s3
for region in "${REGIONS[@]}"; do
    [[ -n "$region" ]] && discover_region "$region"
done

# ── Render inventory ─────────────────────────────────────────────────
render_inventory() {
    echo
    echo "Account:   $ACCOUNT_ID"
    echo "Regions:   ${REGIONS[*]}"
    echo "Prefixes:  ${PREFIXES[*]}"
    echo "Mode:      ${MODE}"

    header "EC2 Instances"
    if (( ${#INSTANCES_FLAT[@]} > 0 )); then
        printf '  %-12s %-21s %-44s %s\n' REGION INSTANCE-ID NAME STATE
        for line in "${INSTANCES_FLAT[@]}"; do
            IFS='|' read -r r iid name state <<< "$line"
            printf '  %-12s %-21s %-44s %s\n' "$r" "$iid" "$name" "$state"
        done
    fi

    header "IAM Roles"
    (( ${#ROLES[@]} > 0 )) && printf '  %s\n' "${ROLES[@]}"

    header "IAM Users"
    (( ${#USERS[@]} > 0 )) && printf '  %s\n' "${USERS[@]}"

    header "IAM Instance Profiles"
    (( ${#PROFILES[@]} > 0 )) && printf '  %s\n' "${PROFILES[@]}"

    header "S3 Buckets"
    (( ${#BUCKETS[@]} > 0 )) && printf '  %s\n' "${BUCKETS[@]}"

    header "CodeDeploy Applications"
    if (( ${#APPS_FLAT[@]} > 0 )); then
        for line in "${APPS_FLAT[@]}"; do
            IFS='|' read -r r app <<< "$line"
            printf '  %-12s %s\n' "$r" "$app"
        done
    fi

    header "CodeDeploy Deployment Groups"
    if (( ${#DGS_FLAT[@]} > 0 )); then
        for line in "${DGS_FLAT[@]}"; do
            IFS='|' read -r r app dg <<< "$line"
            printf '  %-12s %s / %s\n' "$r" "$app" "$dg"
        done
    fi

    header "CodeDeploy On-Premises Instances"
    if (( ${#ONPREM_FLAT[@]} > 0 )); then
        for line in "${ONPREM_FLAT[@]}"; do
            IFS='|' read -r r name <<< "$line"
            printf '  %-12s %s\n' "$r" "$name"
        done
    fi

    local total=$(( ${#INSTANCES_FLAT[@]} + ${#ROLES[@]} + ${#USERS[@]} + ${#PROFILES[@]} \
        + ${#BUCKETS[@]} + ${#APPS_FLAT[@]} + ${#DGS_FLAT[@]} + ${#ONPREM_FLAT[@]} ))

    echo
    rule
    echo "  Summary"
    rule
    printf '  %-30s %d\n' "EC2 instances (running):"  "${#INSTANCES_FLAT[@]}"
    printf '  %-30s %d\n' "IAM roles:"                "${#ROLES[@]}"
    printf '  %-30s %d\n' "IAM users:"                "${#USERS[@]}"
    printf '  %-30s %d\n' "Instance profiles:"        "${#PROFILES[@]}"
    printf '  %-30s %d\n' "S3 buckets:"               "${#BUCKETS[@]}"
    printf '  %-30s %d\n' "CodeDeploy applications:"  "${#APPS_FLAT[@]}"
    printf '  %-30s %d\n' "Deployment groups:"        "${#DGS_FLAT[@]}"
    printf '  %-30s %d\n' "On-prem registrations:"    "${#ONPREM_FLAT[@]}"
    echo
    if (( total == 0 )); then
        echo "  ✅  Nothing found."
    else
        echo "  Found ${total} resource(s) matching:  ${PREFIXES[*]}"
    fi
}

if [[ "$MODE" == "list" ]]; then
    render_inventory
    exit 0
fi

# ── Dry-run / delete ─────────────────────────────────────────────────
# Render the plan in the same dependency order the delete will use, so
# dry-run output and delete output stay aligned.

render_plan() {
    echo
    rule
    echo "  Deletion plan (in dependency order)"
    rule
    local n=1

    if (( ${#INSTANCES_FLAT[@]} > 0 )); then
        printf '  %d. Terminate EC2 instances (and wait for terminated):\n' "$n"; n=$((n+1))
        for line in "${INSTANCES_FLAT[@]}"; do
            IFS='|' read -r r iid name state <<< "$line"
            printf '       %s in %s (%s)\n' "$iid" "$r" "$name"
        done
    fi
    if (( ${#ONPREM_FLAT[@]} > 0 )); then
        printf '  %d. Deregister CodeDeploy on-prem instances:\n' "$n"; n=$((n+1))
        for line in "${ONPREM_FLAT[@]}"; do
            IFS='|' read -r r name <<< "$line"
            printf '       %s in %s\n' "$name" "$r"
        done
    fi
    if (( ${#APPS_FLAT[@]} > 0 )); then
        printf '  %d. Delete CodeDeploy applications (cascades to deployment groups):\n' "$n"; n=$((n+1))
        for line in "${APPS_FLAT[@]}"; do
            IFS='|' read -r r app <<< "$line"
            printf '       %s in %s\n' "$app" "$r"
        done
    fi
    if (( ${#PROFILES[@]} > 0 )); then
        printf '  %d. Detach roles from instance profiles, delete instance profiles:\n' "$n"; n=$((n+1))
        printf '       %s\n' "${PROFILES[@]}"
    fi
    if (( ${#ROLES[@]} > 0 )); then
        printf '  %d. Delete inline + attached policies, delete IAM roles:\n' "$n"; n=$((n+1))
        printf '       %s\n' "${ROLES[@]}"
    fi
    if (( ${#USERS[@]} > 0 )); then
        printf '  %d. Delete access keys, inline policies, delete IAM users:\n' "$n"; n=$((n+1))
        printf '       %s\n' "${USERS[@]}"
    fi
    if (( ${#BUCKETS[@]} > 0 )); then
        printf '  %d. Empty + delete S3 buckets:\n' "$n"; n=$((n+1))
        printf '       %s\n' "${BUCKETS[@]}"
    fi
    echo
}

render_inventory
render_plan

if [[ "$MODE" == "dry-run" ]]; then
    echo "  Dry-run complete. Re-run with --delete to actually remove these."
    exit 0
fi

# ── Delete (real) ────────────────────────────────────────────────────
if (( ASSUME_YES == 0 )); then
    echo
    read -r -p "Type 'delete' to confirm destruction of the resources above: " ans
    [[ "$ans" == "delete" ]] || die "Aborted (got: '${ans}')"
fi

# 1. Terminate EC2 instances. Group by region for batch waits.
if (( ${#INSTANCES_FLAT[@]} > 0 )); then
    declare -A INSTANCES_BY_REGION
    for line in "${INSTANCES_FLAT[@]}"; do
        IFS='|' read -r r iid _ _ <<< "$line"
        INSTANCES_BY_REGION[$r]+=" ${iid}"
    done
    for r in "${!INSTANCES_BY_REGION[@]}"; do
        ids=()
        read -ra ids <<< "${INSTANCES_BY_REGION[$r]}"
        info "Terminating in ${r}: ${ids[*]}"
        aws ec2 terminate-instances --region "$r" --instance-ids "${ids[@]}" >/dev/null || true
    done
    for r in "${!INSTANCES_BY_REGION[@]}"; do
        ids=()
        read -ra ids <<< "${INSTANCES_BY_REGION[$r]}"
        info "Waiting for terminated in ${r}..."
        aws ec2 wait instance-terminated --region "$r" --instance-ids "${ids[@]}" 2>/dev/null || true
    done
fi

# 2. Deregister on-prem instances.
for line in "${ONPREM_FLAT[@]}"; do
    IFS='|' read -r r name <<< "$line"
    info "Deregistering on-prem ${name} in ${r}"
    # remove-tags is best effort (can fail if no matching tag — ignore).
    for p in "${PREFIXES[@]}"; do
        aws deploy remove-tags-from-on-premises-instances --region "$r" \
            --instance-names "$name" --tags "Key=codedeploy-agent-test,Value=*" 2>/dev/null || true
        aws deploy remove-tags-from-on-premises-instances --region "$r" \
            --instance-names "$name" --tags "Key=${p%-}" 2>/dev/null || true
    done
    aws deploy deregister-on-premises-instance --region "$r" --instance-name "$name" 2>/dev/null || \
        warn "deregister failed: ${name} (${r})"
done

# 3. Delete CodeDeploy applications (cascades to deployment groups).
for line in "${APPS_FLAT[@]}"; do
    IFS='|' read -r r app <<< "$line"
    info "Deleting CodeDeploy app ${app} in ${r}"
    aws deploy delete-application --region "$r" --application-name "$app" 2>/dev/null || \
        warn "delete-application failed: ${app} (${r})"
done

# 4. Detach roles from instance profiles, delete instance profiles.
for profile in "${PROFILES[@]}"; do
    info "Cleaning instance profile ${profile}"
    role_in_profile=$(aws iam get-instance-profile --instance-profile-name "$profile" \
        --query 'InstanceProfile.Roles[0].RoleName' --output text 2>/dev/null || echo "")
    if [[ -n "$role_in_profile" && "$role_in_profile" != "None" ]]; then
        aws iam remove-role-from-instance-profile \
            --instance-profile-name "$profile" --role-name "$role_in_profile" 2>/dev/null || true
    fi
    aws iam delete-instance-profile --instance-profile-name "$profile" 2>/dev/null || \
        warn "delete-instance-profile failed: ${profile}"
done

# 5. Delete IAM role policies + roles.
for role in "${ROLES[@]}"; do
    info "Cleaning role ${role}"
    # Inline role policies.
    for pol in $(aws iam list-role-policies --role-name "$role" --query 'PolicyNames[]' --output text 2>/dev/null); do
        aws iam delete-role-policy --role-name "$role" --policy-name "$pol" 2>/dev/null || true
    done
    # Managed-policy attachments.
    for arn in $(aws iam list-attached-role-policies --role-name "$role" --query 'AttachedPolicies[].PolicyArn' --output text 2>/dev/null); do
        aws iam detach-role-policy --role-name "$role" --policy-arn "$arn" 2>/dev/null || true
    done
    aws iam delete-role --role-name "$role" 2>/dev/null || \
        warn "delete-role failed: ${role}"
done

# 6. Delete IAM access keys + user policies + users.
for user in "${USERS[@]}"; do
    info "Cleaning user ${user}"
    for key in $(aws iam list-access-keys --user-name "$user" --query 'AccessKeyMetadata[].AccessKeyId' --output text 2>/dev/null); do
        aws iam delete-access-key --user-name "$user" --access-key-id "$key" 2>/dev/null || true
    done
    for pol in $(aws iam list-user-policies --user-name "$user" --query 'PolicyNames[]' --output text 2>/dev/null); do
        aws iam delete-user-policy --user-name "$user" --policy-name "$pol" 2>/dev/null || true
    done
    for arn in $(aws iam list-attached-user-policies --user-name "$user" --query 'AttachedPolicies[].PolicyArn' --output text 2>/dev/null); do
        aws iam detach-user-policy --user-name "$user" --policy-arn "$arn" 2>/dev/null || true
    done
    aws iam delete-user --user-name "$user" 2>/dev/null || \
        warn "delete-user failed: ${user}"
done

# 7. Empty + delete S3 buckets.
for bucket in "${BUCKETS[@]}"; do
    info "Emptying + deleting bucket ${bucket}"
    aws s3 rm "s3://${bucket}" --recursive 2>/dev/null || true
    aws s3api delete-bucket --bucket "$bucket" 2>/dev/null || \
        warn "delete-bucket failed: ${bucket}"
done

# 8. Clean up local state files matching the prefixes.
shopt -s nullglob
for f in /tmp/codedeploy-agent-*-state.json; do
    info "Removing local state ${f}"
    rm -f "$f"
done
shopt -u nullglob

echo
rule
echo "  Cleanup complete."
rule
echo "  Re-run without --delete to verify nothing remains."
