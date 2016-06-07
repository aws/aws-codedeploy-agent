#!/usr/bin/env bash

# post-removal script for github.com/bdashrad/aws-codedeploy-agent

install_path=/opt/aws-codedeploy-agent

echo
read -r -p "Remove '/opt/aws-codedeploy-agent'? [y/N] " response

case $response in
  [yY][eE][sS]|[yY])
    rm -rf ${install_path}/.bundle
    rm -rf ${install_path}/vendor/cache
    rm -rf ${install_path}/vendor/ruby
    rm -rf ${install_path}/state
    ;;
  *)
    echo "'/opt/code-deploy-agent' not removed"
    ;;
esac
