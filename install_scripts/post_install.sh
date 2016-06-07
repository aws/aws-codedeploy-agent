#!/usr/bin/env bash

type gem > /dev/null
if [ $? != 0 ]; then
  # install ruby
  apt install -y -o Dpkg::Options::="--force-confold" -o Dpkg::Options::="--force-confdef" ruby
fi

gem install --no-ri --no-rdoc bundler
cd /opt/codedeploy-agent || exit 1
runuser -l ubuntu -c 'cd /opt/codedeploy-agent && bundle install'
if [ $? = 0 ]; then
  echo "Installed."
else
  echo
  echo -e "\033[0;31mError installing gems. Please run the following command as a non-root user:\033[0m"
  echo
  echo "cd /opt/codedeploy-agent && bundle install"
  echo
  echo "Then restart the 'codedeploy-agent' service."
  echo
fi
