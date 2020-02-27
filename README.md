# AWS CodeDeploy Agent

[![Code Climate](https://codeclimate.com/github/aws/aws-codedeploy-agent.png)](https://codeclimate.com/github/aws/aws-codedeploy-agent) [![Build Status](https://travis-ci.org/aws/aws-codedeploy-agent.png?branch=master)](https://travis-ci.org/aws/aws-codedeploy-agent) [![Coverage Status](https://coveralls.io/repos/aws/aws-codedeploy-agent/badge.svg?branch=master&service=github)](https://coveralls.io/r/aws/aws-codedeploy-agent?branch=master)


## Build Steps

``` ruby
git clone https://github.com/aws/aws-codedeploy-agent.git
gem install bundler -v 1.3.5
cd aws-codedeploy-agent
bundle install
rake clean && rake
```

## Starting up the CodeDeploy Agent Locally for manual testing

`bin/codedeploy-agent start`

To stop it:

`bin/codedeploy-agent stop`

## Integration Test

Please do the build steps mentioned above before running the integration test.

The integration test creates the following
* An IAM role "codedeploy-agent-integ-test-deployment-role" if it doesn't exist
* An IAM role "codedeploy-agent-integ-test-instance-role" if it doesn't exist
* A CodeDeploy application
* Startup the codedeploy agent on your host
* A CodeDeploy deployment group with your host in it
* A CodeDeploy deployment to your host.
* Local Deployments to your host.

It terminates the test ec2 instance and deletes the CodeDeploy application at the end of each test run.
It also terminates any test ec2 instances before starting up the test.

Create your default aws credentials file in the default location (~/.aws/credentials on linux/mac and %USERPROFILE%.awscredentials on windows). Add your AWS access key, secret key, and optionally your session token there. The access key should have permission to create the above mentioned resources. You can also change the default region. Note that temporary credentials won't work. 

Sample format of the credentials file:

```
[default]
aws_access_key_id=<keyID>
aws_secret_access_key=<key>
```

To run the integration test execute:

```
rake test-integration
```
