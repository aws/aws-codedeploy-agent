# AWS CodeDeploy Agent

[![Code Climate](https://codeclimate.com/github/aws/aws-codedeploy-agent.png)](https://codeclimate.com/github/aws/aws-codedeploy-agent) [![Build Status](https://travis-ci.org/aws/aws-codedeploy-agent.png?branch=master)](https://travis-ci.org/aws/aws-codedeploy-agent) [![Coverage Status](https://coveralls.io/repos/aws/aws-codedeploy-agent/badge.svg?branch=master&service=github)](https://coveralls.io/r/aws/aws-codedeploy-agent?branch=master)


## Build Steps

``` ruby
git clone https://github.com/aws/aws-codedeploy-agent.git
gem install bundler
cd aws-codedeploy-agent
bundle install
rake clean && rake
```

## Integration Test
  
Please do the build steps mentioned above before running the integration test.
  
The integration test creates the following
* An IAM role "codedeploy-agent-integ-test-deployment-role" if it doesn't exist
* An IAM role "codedeploy-agent-integ-test-instance-role" if it doesn't exist
* A CodeDeploy application
* A CodeDeploy deployment group
* An EC2 key pair "codedeploy-agent-integ-test-key" if it doesn't exist
* An EC2 security group "codedeploy-agent-integ-test-sg" if it doesn't exist
* An EC2 instance tagged with key "codedeploy-agent-integ-test-instance"
* A CodeDeploy deployment on that ec2 instance.
  
It terminates the test ec2 instance and deletes the CodeDeploy application at the end of each test run.
It also terminates any test ec2 instances before starting up the test.
  
Update the features/AwsCredentials.yml file with AWS access key and secret key. The access key should have permission to create the above mentioned resources. You can also change the default region and ami id if you want. To run the integration test execute
  
```
rake test-integration
```
