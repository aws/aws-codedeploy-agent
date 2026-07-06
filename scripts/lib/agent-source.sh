#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────
# scripts/lib/agent-source.sh — locate a pre-built agent binary in S3.
#
# Source this from another script. The script must already define
# `info()` (a logger) and `die()` (a fatal-error printer that exits).
#
#   SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
#   source "${SCRIPT_DIR}/lib/agent-source.sh"
#
# ── Caller-facing environment variables ──
#
#   AGENT_S3_URI
#       Full s3://bucket/key URI to a specific pre-built binary. Used
#       verbatim. The caller is responsible for ensuring the file
#       matches the script's target platform (Linux ELF vs Windows EXE).
#
#   AGENT_S3_PREFIX
#       s3://bucket/prefix; helpers append the platform-specific suffix:
#         /linux/codedeploy-agent       (platform=linux)
#         /windows/codedeploy-agent.exe (platform=windows)
#       Set this when the same prefix should drive both Linux and
#       Windows scripts (e.g. a release-staging bucket).
#
# AGENT_S3_URI takes precedence over AGENT_S3_PREFIX when both are set.
#
# ── Helpers ──
#
#   agent_s3_uri_for <platform>
#       Echo the resolved S3 URI for the platform, or empty string if
#       neither AGENT_S3_URI nor AGENT_S3_PREFIX is set. <platform> is
#       one of: linux, windows.
#
#   using_prebuilt_agent <platform>
#       Returns 0 (success) when a pre-built URI is configured for the
#       platform; non-zero otherwise. Use as a boolean in `if` blocks.
#
#   ensure_local_agent <platform> <dest_path>
#       If a pre-built URI is configured for the platform, fetch it via
#       `aws s3 cp` into <dest_path>, chmod +x, and return. Otherwise
#       verify <dest_path> already exists. die()s if neither succeeds.
#       Idempotent: repeated calls with the same dest are a no-op when
#       the file is already present and no override is set.
# ──────────────────────────────────────────────────────────────────────

agent_s3_uri_for() {
    local platform="${1:?platform required (linux|windows)}"
    if [[ -n "${AGENT_S3_URI:-}" ]]; then
        printf '%s' "$AGENT_S3_URI"
        return 0
    fi
    if [[ -n "${AGENT_S3_PREFIX:-}" ]]; then
        case "$platform" in
            linux)   printf '%s/linux/codedeploy-agent'        "$AGENT_S3_PREFIX" ;;
            windows) printf '%s/windows/codedeploy-agent.exe'  "$AGENT_S3_PREFIX" ;;
            *) die "agent_s3_uri_for: unknown platform '$platform'" ;;
        esac
    fi
    return 0
}

using_prebuilt_agent() {
    local platform="${1:?platform required (linux|windows)}"
    local uri
    uri=$(agent_s3_uri_for "$platform")
    [[ -n "$uri" ]]
}

ensure_local_agent() {
    local platform="${1:?platform required (linux|windows)}"
    local dest="${2:?dest path required}"
    local uri
    uri=$(agent_s3_uri_for "$platform")
    if [[ -n "$uri" ]]; then
        info "Fetching pre-built agent from ${uri}"
        mkdir -p "$(dirname "$dest")"
        aws s3 cp "$uri" "$dest" >/dev/null \
            || die "Failed to fetch agent from ${uri}"
        chmod +x "$dest"
    fi
    [[ -x "$dest" ]] || die "Agent binary not found at ${dest} — set AGENT_S3_URI or AGENT_S3_PREFIX, or run 'cargo build --release' first"
}
