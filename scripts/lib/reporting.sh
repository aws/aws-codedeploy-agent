#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# scripts/lib/reporting.sh — shared deployment-status reporting helpers.
#
# Source this file from another script. The script must already define
# `info()` (a logger) and `die()` (a fatal-error printer that exits).
#
#   SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
#   source "${SCRIPT_DIR}/lib/reporting.sh"
#
# Helpers exposed here:
#
#   print_banner <OUTCOME> <title> [detail lines...]
#       Print a uniform fixed-width banner. OUTCOME is a free-form
#       label (PASS / FAIL / REPRODUCED / NOT_REPRODUCED / SKIPPED / …).
#       Caller decides the exit code.
#
#   print_deployment_table <region> <id1> [id2 …]
#       Print one row per deployment ID with status + error code, in
#       a uniform column layout.
#
#   wait_for_deployments <region> <timeout_seconds> <id1> [id2 …]
#       Poll every 5s until every deployment is in
#       {Succeeded, Failed, Stopped} or the timeout is reached.
#       Returns 0 on terminal completion, non-zero on timeout.
#       Populates these globals in the caller's shell:
#           DEPLOY_TOTAL DEPLOY_SUCCEEDED DEPLOY_FAILED
#           DEPLOY_STOPPED DEPLOY_PENDING DEPLOY_ELAPSED
#
#   print_deployment_outcome <SUCCEEDED|FAILED>
#       Prints a marquee banner intended to be impossible to miss in
#       CI logs. Designed to be called from the deployment-tracking
#       command (cmd_status) right after polling completes, so that
#       both `status` and any composite command (`all`) emit the same
#       DEPLOYMENT SUCCEEDED / DEPLOYMENT FAILED line *before* teardown.
#
#       Side effect: writes the outcome to ${DEPLOYMENT_OUTCOME_FILE}
#       (defaults to /tmp/.deployment-outcome-$$) so cmd_all can replay
#       it as the very last line before teardown via
#       replay_deployment_outcome.
#
#   replay_deployment_outcome
#       Reads ${DEPLOYMENT_OUTCOME_FILE} and prints the same marquee
#       banner. No-op if the file is missing. Intended to run in
#       cmd_all right after the agent-log dump, so the outcome is the
#       last thing the user sees before teardown.
#
# All helpers are idempotent and have no side effects on AWS state.
# ──────────────────────────────────────────────────────────────────────

# Width of the banner rule. Banner content lines are not constrained.
: "${REPORTING_BANNER_WIDTH:=64}"

# Internal: build a `═` rule of REPORTING_BANNER_WIDTH characters.
_reporting_rule() {
    local i out=""
    for ((i = 0; i < REPORTING_BANNER_WIDTH; i++)); do out+="═"; done
    printf '%s' "$out"
}

# print_banner <outcome> <title> [detail line ...]
print_banner() {
    local outcome="${1:-?}" title="${2:-}"
    [[ $# -gt 0 ]] && shift
    [[ $# -gt 0 ]] && shift
    local rule
    rule="$(_reporting_rule)"

    echo ""
    echo "$rule"
    printf '  [%s] %s\n' "$outcome" "$title"
    echo "$rule"
    while [[ $# -gt 0 ]]; do
        printf '  %s\n' "$1"
        shift
    done
    echo ""
}

# print_deployment_table <region> <id1> [id2 ...]
print_deployment_table() {
    local region="$1"; shift
    [[ $# -gt 0 ]] || return 0

    printf '  %-22s %-12s %s\n' "DEPLOYMENT_ID" "STATUS" "ERROR"
    printf '  %-22s %-12s %s\n' "----------------------" "------------" "----------------------------------------"

    local id info status err
    for id in "$@"; do
        info=$(aws deploy get-deployment --deployment-id "$id" \
            --region "$region" \
            --query 'deploymentInfo.{s:status,e:errorInformation.code}' \
            --output json 2>/dev/null) || info='{}'
        status=$(echo "$info" | jq -r '.s // "?"' 2>/dev/null)
        err=$(echo "$info" | jq -r '.e // empty' 2>/dev/null)
        [[ -z "$err" || "$err" == "None" || "$err" == "null" ]] && err="—"
        printf '  %-22s %-12s %s\n' "$id" "$status" "$err"
    done
}

# wait_for_deployments <region> <timeout_seconds> <id1> [id2 ...]
wait_for_deployments() {
    local region="$1" timeout="$2"; shift 2

    DEPLOY_TOTAL=$#
    DEPLOY_SUCCEEDED=0
    DEPLOY_FAILED=0
    DEPLOY_STOPPED=0
    DEPLOY_PENDING=0
    # shellcheck disable=SC2034  # consumed by callers via documented contract
    DEPLOY_ELAPSED=0

    if (( DEPLOY_TOTAL == 0 )); then
        return 0
    fi

    local elapsed=0 line id status
    while :; do
        DEPLOY_SUCCEEDED=0
        DEPLOY_FAILED=0
        DEPLOY_STOPPED=0
        DEPLOY_PENDING=0
        line=""
        for id in "$@"; do
            status=$(aws deploy get-deployment --deployment-id "$id" \
                --region "$region" --query 'deploymentInfo.status' \
                --output text 2>/dev/null) || status="?"
            line+="${id:0:14}=${status} "
            case "$status" in
                Succeeded) DEPLOY_SUCCEEDED=$((DEPLOY_SUCCEEDED + 1)) ;;
                Failed)    DEPLOY_FAILED=$((DEPLOY_FAILED + 1)) ;;
                Stopped)   DEPLOY_STOPPED=$((DEPLOY_STOPPED + 1)) ;;
                *)         DEPLOY_PENDING=$((DEPLOY_PENDING + 1)) ;;
            esac
        done
        printf '  [%4ds] %s\n' "$elapsed" "$line"

        # shellcheck disable=SC2034  # consumed by callers via documented contract
        DEPLOY_ELAPSED="$elapsed"
        if (( DEPLOY_PENDING == 0 )); then
            return 0
        fi
        if (( elapsed >= timeout )); then
            return 1
        fi
        sleep 5
        elapsed=$((elapsed + 5))
    done
}

: "${DEPLOYMENT_OUTCOME_FILE:=/tmp/.deployment-outcome-$$}"

# print_deployment_outcome <SUCCEEDED|FAILED>
print_deployment_outcome() {
    local outcome="${1:?outcome required (SUCCEEDED or FAILED)}"
    local rule
    rule="$(_reporting_rule)"
    echo ""
    echo "$rule"
    echo "$rule"
    case "$outcome" in
        SUCCEEDED) printf '   ✅  DEPLOYMENT SUCCEEDED\n' ;;
        FAILED)    printf '   ❌  DEPLOYMENT FAILED\n' ;;
        *)         printf '   ?  DEPLOYMENT %s\n' "$outcome" ;;
    esac
    echo "$rule"
    echo "$rule"
    echo ""

    printf '%s\n' "$outcome" > "$DEPLOYMENT_OUTCOME_FILE" 2>/dev/null || true
}

# replay_deployment_outcome
replay_deployment_outcome() {
    [[ -s "$DEPLOYMENT_OUTCOME_FILE" ]] || return 0
    local outcome
    outcome=$(<"$DEPLOYMENT_OUTCOME_FILE")
    local rule
    rule="$(_reporting_rule)"
    echo ""
    echo "$rule"
    echo "$rule"
    case "$outcome" in
        SUCCEEDED) printf '   ✅  DEPLOYMENT SUCCEEDED\n' ;;
        FAILED)    printf '   ❌  DEPLOYMENT FAILED\n' ;;
        *)         printf '   ?  DEPLOYMENT %s\n' "$outcome" ;;
    esac
    echo "$rule"
    echo "$rule"
    echo ""
}
