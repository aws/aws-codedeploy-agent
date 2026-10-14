#!/bin/sh
# Uninstall the AWS CodeDeploy agent on macOS.
#
# .pkg has no uninstall hook; this is the rpm %preun / deb prerm counterpart:
#   sudo /opt/codedeploy-agent/bin/uninstall.sh
# Deployment state, logs and the operator's config are left in place, as the
# Linux packages do; only the .pkgnew template copy is removed.
set -eu

LABEL=com.amazon.aws.codedeploy-agent
PLIST=/Library/LaunchDaemons/$LABEL.plist

if launchctl print "system/$LABEL" >/dev/null 2>&1; then
    launchctl bootout "system/$LABEL" || :
    # bootout only sends SIGTERM; wait (up to ExitTimeOut) for the job to
    # unload before removing the binary it may still re-exec.
    i=0
    while launchctl print "system/$LABEL" >/dev/null 2>&1; do
        i=$((i + 1))
        if [ "$i" -ge 7260 ]; then   # ExitTimeOut + margin: let launchd's own SIGKILL land first
            echo "$LABEL did not exit within ExitTimeOut (7200 s); aborting uninstall" >&2
            exit 1
        fi
        sleep 1
    done
fi
launchctl disable "system/$LABEL" || :

pkgutil --forget "$LABEL" >/dev/null 2>&1 || :

rm -f "$PLIST"
rm -f /opt/codedeploy-agent/bin/codedeploy-local
rm -f /opt/codedeploy-agent/bin/codedeploy-agent
rm -f /opt/codedeploy-agent/bin/uninstall.sh   # this script; sh keeps its own fd open
rmdir /opt/codedeploy-agent/bin 2>/dev/null || :
# Keep codedeployagent.yml; drop only the .pkgnew template copy.
rm -f /etc/codedeploy-agent/conf/codedeployagent.yml.pkgnew
rmdir /etc/codedeploy-agent/conf /etc/codedeploy-agent 2>/dev/null || :

echo "AWS CodeDeploy agent removed."
