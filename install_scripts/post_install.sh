#!/usr/bin/env bash
# post install script for github.com/bdashrad/aws-codedeploy-agent

type bundler > /dev/null
if [ $? != 0 ]; then
  type gem > /dev/null
  if [ $? != 0 ]; then
    # install ruby
    apt install -y -o Dpkg::Options::="--force-confold" -o Dpkg::Options::="--force-confdef" ruby
  fi
  # install bundler
  gem install --no-ri --no-rdoc bundler
fi

cd /opt/codedeploy-agent || exit 1
# runuser -l ubuntu -c 'cd /opt/codedeploy-agent && bundle install --without test build --path vendor/'
bundle install --without test build --path vendor/
if [ $? = 0 ]; then
  echo "Installed."
else
  echo
  echo -e "\033[0;31mError installing gems. Please run the following command:\033[0m"
  echo
  echo "cd /opt/codedeploy-agent && sudo bundle install --without test build --path vendor/"
  echo
  echo "Then restart the 'codedeploy-agent' service."
  echo
fi
