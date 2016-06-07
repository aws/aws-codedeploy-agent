#!/usr/bin/env bash
# post-removal script for github.com/bdashrad/aws-codedeploy-agent

install_path=/opt/aws-codedeploy-agent

echo "Stopping codedeploy-agent service..."
service codedeploy-agent stop

echo
read -r -p "Remove '/opt/aws-codedeploy-agent'? [y/N] " response

case $response in
  [yY][eE][sS]|[yY])
    for path in .bundle vendor state; do
      echo "Removing ${install_path}/${path}"
      sudo rm -rf "${install_path:?}/${path:?}"
    done
    ;;
  *)
    echo "'/opt/code-deploy-agent' not removed"
    ;;
esac
