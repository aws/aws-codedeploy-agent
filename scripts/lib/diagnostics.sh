#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# scripts/lib/diagnostics.sh — make silent script exits visible.
#
# Source this from another script. The script must already define
# `info()` and have `set -euo pipefail` enabled.
#
#   SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
#   source "${SCRIPT_DIR}/lib/diagnostics.sh"
#   diagnostics_install
#
# What this provides:
#
#   - Always-on EXIT trap that announces the script's exit code and
#     the line number where the script exited. If the exit code is
#     non-zero and no error message has already been printed, you'll
#     at least see WHY the shell terminated.
#
#   - DEBUG=1 environment toggle. Set DEBUG=1 (any non-empty value)
#     before running the script and the EXIT trap will also dump the
#     bash call stack at exit, plus enable `set -x` from the moment
#     diagnostics_install is called.
#
#   - Force AWS_PAGER='' globally so a user-configured pager (less,
#     more, etc.) cannot hang the AWS CLI and silently kill the script
#     via SIGPIPE.
#
# ── Cooperating with the script's `die`
#
# When a script calls `die` to abort with its own user-facing error
# message, the EXIT trap would otherwise add a redundant
# "exited with code X (no ERR trace; …)" line. To suppress that, have
# `die` set DIAG_USER_EXIT=1 just before its `exit`:
#
#   die() { echo "ERROR: $*" >&2; DIAG_USER_EXIT=1; exit 1; }
#
# The trap detects the flag and stays silent, so the user only sees
# the message you wrote. DEBUG=1 still prints the call stack.
# ──────────────────────────────────────────────────────────────────────

# Prevent a user-configured pager from hanging the AWS CLI silently.
export AWS_PAGER=''

diagnostics_install() {
    local script_name
    script_name="$(basename "${BASH_SOURCE[1]:-$0}")"

    # Propagate ERR traps into functions and subshells.
    set -E

    # Record the line and command of the most recent non-zero exit.
    # shellcheck disable=SC2154
    trap '_DIAG_LAST_LINE=$LINENO; _DIAG_LAST_CMD=$BASH_COMMAND' ERR

    # shellcheck disable=SC2154
    trap 'rc=$?; _diagnostics_on_exit "'"$script_name"'" "$rc"' EXIT

    if [[ -n "${DEBUG:-}" ]]; then
        info "DEBUG mode: enabling 'set -x' tracing"
        set -x
    fi
}

_diagnostics_on_exit() {
    local script_name="$1" rc="$2"

    if (( rc == 0 )); then
        return 0
    fi

    echo "" >&2
    if [[ -n "${_DIAG_LAST_LINE:-}" ]]; then
        printf '!! %s exited with code %d at line %s\n' \
            "$script_name" "$rc" "$_DIAG_LAST_LINE" >&2
        if [[ -n "${_DIAG_LAST_CMD:-}" ]]; then
            printf '!! failing command: %s\n' "$_DIAG_LAST_CMD" >&2
        fi
    else
        printf '!! %s exited with code %d (no ERR trace; likely an explicit exit or signal)\n' \
            "$script_name" "$rc" >&2
    fi

    if [[ -n "${DEBUG:-}" ]]; then
        echo "!! call stack:" >&2
        local i
        for ((i = 1; i < ${#BASH_SOURCE[@]}; i++)); do
            printf '!!   #%d %s:%d %s\n' \
                "$i" \
                "${BASH_SOURCE[$i]:-?}" \
                "${BASH_LINENO[$i - 1]:-?}" \
                "${FUNCNAME[$i]:-MAIN}" >&2
        done
    fi

    return 0
}
