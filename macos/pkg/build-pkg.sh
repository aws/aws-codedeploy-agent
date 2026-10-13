#!/bin/sh
# Build the macOS installer package (.pkg) for one architecture.
#
# Run after `cargo build --release --target <triple>`.
# Runs on macOS only (pkgbuild/productbuild are Xcode command-line tools).
#
# Usage:
#   macos/pkg/build-pkg.sh <binary> <arch> <version> <full-version> <out-dir>
#     binary        path to the release codedeploy-agent binary for <arch>
#     arch          x86_64 | aarch64  (matches LATEST_VERSION pkg_<arch> keys)
#     version       bare agent version, e.g. 2.1.1  (package version)
#     full-version  version-build, e.g. 2.1.1-1234  (file name only)
#     out-dir       directory to write codedeploy-agent-<full-version>.<arch>.pkg
#
# Payload (install-location /):
#   /opt/codedeploy-agent/bin/codedeploy-agent                        0755
#   /opt/codedeploy-agent/bin/codedeploy-local -> codedeploy-agent    (symlink; see legacy_local_argv in src/main.rs)
#   /private/etc/codedeploy-agent/conf/codedeployagent.yml            0644 (kept on upgrade, see scripts/)
#   /Library/LaunchDaemons/com.amazon.aws.codedeploy-agent.plist       0644
# (/etc is a symlink to /private/etc on macOS; the payload uses the real path.)
# Signing and notarization are separate steps, not done here.
set -eu

if [ "$#" -ne 5 ]; then
    sed -n '2,/^set -eu/p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
fi

BINARY=$1
ARCH=$2
VERSION=$3
FULL_VERSION=$4
OUT_DIR=$5

case "$ARCH" in
    x86_64)  HOST_ARCH=x86_64 ;;
    aarch64) HOST_ARCH=arm64 ;;   # Distribution.xml uses Apple's name
    *) echo "arch must be x86_64 or aarch64, got '$ARCH'" >&2; exit 2 ;;
esac
[ -f "$BINARY" ] || { echo "binary not found: $BINARY" >&2; exit 2; }

HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
IDENTIFIER=com.amazon.aws.codedeploy-agent
MIN_OS=15.0   # oldest supported macOS; must match MACOSX_DEPLOYMENT_TARGET

WORK=$(mktemp -d "${TMPDIR:-/tmp}/codedeploy-pkg.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
ROOT="$WORK/root"

# --- payload ---------------------------------------------------------------
install -d -m 0755 "$ROOT/opt/codedeploy-agent/bin"
install -m 0755 "$BINARY" "$ROOT/opt/codedeploy-agent/bin/codedeploy-agent"
ln -s codedeploy-agent "$ROOT/opt/codedeploy-agent/bin/codedeploy-local"
# pkg has no uninstall hook; ship the manual uninstaller next to the binary.
install -m 0755 "$HERE/uninstall.sh" "$ROOT/opt/codedeploy-agent/bin/uninstall.sh"

install -d -m 0755 "$ROOT/private/etc/codedeploy-agent/conf"
install -m 0644 "$REPO/conf/codedeployagent.yml" "$ROOT/private/etc/codedeploy-agent/conf/codedeployagent.yml"

install -d -m 0755 "$ROOT/Library/LaunchDaemons"
install -m 0644 "$REPO/launchd/$IDENTIFIER.plist" "$ROOT/Library/LaunchDaemons/$IDENTIFIER.plist"

# --- component package -----------------------------------------------------
COMPONENT="$WORK/$IDENTIFIER.pkg"
# installer silently skips non-executable scripts.
chmod 0755 "$HERE/scripts/preinstall" "$HERE/scripts/postinstall"
pkgbuild \
    --root "$ROOT" \
    --install-location / \
    --identifier "$IDENTIFIER" \
    --version "$VERSION" \
    --scripts "$HERE/scripts" \
    --ownership recommended \
    "$COMPONENT"

# --- distribution package --------------------------------------------------
sed -e "s|@VERSION@|$VERSION|g" \
    -e "s|@HOST_ARCH@|$HOST_ARCH|g" \
    -e "s|@MIN_OS@|$MIN_OS|g" \
    "$HERE/Distribution.xml.in" > "$WORK/Distribution.xml"

mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/codedeploy-agent-$FULL_VERSION.$ARCH.pkg"
productbuild \
    --distribution "$WORK/Distribution.xml" \
    --package-path "$WORK" \
    "$OUT"

echo "$OUT"
