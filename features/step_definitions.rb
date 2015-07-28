require 'aws-sdk-core'
require_relative '../features/aws_credentials'
require 'securerandom'
require 'base64'

CODEDEPLOY_TEST_PREFIX = "codedeploy-agent-integ-test-"
EC2_SECURITY_GROUP = "#{CODEDEPLOY_TEST_PREFIX}sg"
EC2_KEY_PAIR = "#{CODEDEPLOY_TEST_PREFIX}key"
EC2_TAG_KEY = "#{CODEDEPLOY_TEST_PREFIX}instance"
DEPLOYMENT_ROLE_NAME = "#{CODEDEPLOY_TEST_PREFIX}deployment-role"
INSTANCE_ROLE_NAME = "#{CODEDEPLOY_TEST_PREFIX}instance-role"

def eventually(options = {}, &block)
  seconds = options[:upto] || 300
  delays = [1]
  while delays.inject(0) { |sum, i| sum + i } < seconds
    delays << [delays.last * 1.2, 60.0].min
  end
  begin
    yield
  rescue StandardError, RSpec::Expectations::ExpectationNotMetError => error
    unless delays.empty?
      sleep(delays.shift)
      retry
    end
    raise error
  end
end

Before("@codedeploy-agent") do
  AwsCredentials.instance.configure
  @codedeploy_client = Aws::CodeDeploy::Client.new
  @ec2_client = Aws::EC2::Client.new
  @iam_client = Aws::IAM::Client.new
  instance_ids = get_test_ec2_instances.collect {|i| i.instance_id}
  @ec2_client.terminate_instances({:instance_ids => instance_ids}) unless instance_ids.empty?
end

After("@codedeploy-agent") do
  @codedeploy_client.delete_application({:application_name => @application_name}) unless @application_name.nil? 
  @ec2_client.terminate_instances({:instance_ids => [@instance_id]}) unless @instance_id.nil?
end

Given(/^I have an application$/) do
  @application_name = "codedeploy-github-testapp-#{SecureRandom.hex(10)}"
  @codedeploy_client.create_application(:application_name => @application_name)
  puts "It is going to take a few minutes to spin up an ec2 instance..."
end

Given(/^I have a deployment group containing a single EC2 AMI with preinstalled new instance agent$/) do
  @deployment_group_name = "codedeploy-github-testdg-#{SecureRandom.hex(10)}"
  create_deployment_role
  create_instance_role
  create_instance_profile
  start_ec2_instance
  create_deployment_group
end

When(/^I create a deployment for the application and deployment group with the test S(\d+) revision$/) do |arg1|
  @deployment_id = @codedeploy_client.create_deployment({:application_name => @application_name,
                            :deployment_group_name => @deployment_group_name,
                            :revision => { :revision_type => "S3",
                                           :s3_location => {
                                             :bucket => "aws-codedeploy-us-east-1",
                                             :key => "samples/latest/SampleApp_Linux.zip",
                                             :bundle_type => "zip"
                                           }
                                         },
                             :deployment_config_name => "CodeDeployDefault.OneAtATime",
                             :description => "CodeDeploy agent github test",
                          }).deployment_id
end

Then(/^the overall deployment should eventually be in progress$/) do
  assert_deployment_status("InProgress", 60)
end

Then(/^the deployment should contain all the instances I tagged$/) do
  instances = @codedeploy_client.list_deployment_instances(:deployment_id => @deployment_id).instances_list
  expect(instances.size).to eq(1)
end

Then(/^the overall deployment should eventually succeed$/) do
  assert_deployment_status("Succeeded", 300)
end

def create_deployment_group
  @codedeploy_client.create_deployment_group({:application_name => @application_name,
                                  :deployment_group_name => @deployment_group_name, 
                                  :ec2_tag_filters => [{:key => EC2_TAG_KEY, :type => "KEY_ONLY"}], 
                                  :service_role_arn => @deployment_role})
end

def get_test_ec2_instances
  reservations = @ec2_client.describe_instances({:filters => [{:name => "tag-key", :values =>[EC2_TAG_KEY]}, {:name => "instance-state-name", :values => ["running", "pending"]}]}).reservations
  instances = []
  for reservation in reservations
    instances += reservation.instances
  end
  instances
end

def start_ec2_instance
  create_key_pair
  create_security_group
  start_and_tag_instance
end

def create_key_pair
  begin
    @ec2_client.create_key_pair({:key_name => EC2_KEY_PAIR})
  rescue Aws::EC2::Errors::InvalidKeyPairDuplicate
    #Use the existing key
  end
  eventually(:upto => 60) do
    expect(@ec2_client.describe_key_pairs({:key_names => [EC2_KEY_PAIR]}).key_pairs).not_to be_empty
  end
end

def create_security_group
  begin
    @security_group_id ||= @ec2_client.create_security_group({:group_name => EC2_SECURITY_GROUP, :description => "CodeDeploy agent integ test instance security group."}).group_id
  rescue Aws::EC2::Errors::InvalidGroupDuplicate
    #Use the existing security group
  end
  eventually(:upto => 60) do 
    security_groups = @ec2_client.describe_security_groups({:group_names => [EC2_SECURITY_GROUP]}).security_groups
    expect(security_groups).not_to be_empty
    @security_group_id ||= security_groups[0].group_id
  end
end

def start_and_tag_instance
  eventually(:upto => 60) do
    @instance_id = @ec2_client.run_instances({
      :image_id => AwsCredentials.instance.ec2_ami,
      :instance_type => "t2.micro",
      :min_count => 1,
      :max_count => 1,
      :key_name => EC2_KEY_PAIR,
      :security_group_ids => [@security_group_id],
      :iam_instance_profile => {:arn => @instance_profile},
      :user_data => get_user_data
    }).instances[0].instance_id
    expect(@instance_id).not_to be_nil
  end
  eventually(:upto => 600) do
    expect(@ec2_client.describe_instance_status(:instance_ids => [@instance_id]).instance_statuses[0].instance_state.name).to eq "running"
  end
  @ec2_client.create_tags({:resources => [@instance_id], :tags => [{:key => EC2_TAG_KEY, :value => ""}]})
end

def get_user_data
  user_data = "#!/bin/bash\n"\
              "yum install -y git gcc ruby-devel\n"\
              "cd /home/ec2-user\n"\
              "gem install io-console\n"\
              "gem install bundler\n"\
              "git clone https://github.com/aws/aws-codedeploy-agent.git\n"\
              "cd aws-codedeploy-agent\n"\
              "BUNDLE=`whereis bundle | cut -d' ' -f 2`\n"\
              "$BUNDLE install\n"\
              "mkdir -p /etc/codedeploy-agent/conf\n"\
              "cp conf/codedeployagent.yml /etc/codedeploy-agent/conf/\n"\
              "$BUNDLE exec bin/codedeploy-agent start"
  Base64.encode64(user_data)
end

def create_deployment_role
  begin
    @iam_client.create_role({:role_name => DEPLOYMENT_ROLE_NAME,
                            :assume_role_policy_document => deployment_role_policy}).role.arn
    @iam_client.attach_role_policy({:role_name => DEPLOYMENT_ROLE_NAME,
                                    :policy_arn => "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole"})
  rescue Aws::IAM::Errors::EntityAlreadyExists
    #Using the existing role
  end
  eventually(:upto => 60) do
    deployment_role = @iam_client.get_role({:role_name => DEPLOYMENT_ROLE_NAME}).role
    expect(deployment_role).not_to be_nil
    @deployment_role ||= deployment_role.arn
  end
end

def create_instance_role
  begin
    @iam_client.create_role({:role_name => INSTANCE_ROLE_NAME,
                            :assume_role_policy_document => instance_role_policy}).role.arn
    @iam_client.attach_role_policy({:role_name => INSTANCE_ROLE_NAME,
                                    :policy_arn => "arn:aws:iam::aws:policy/service-role/AmazonEC2RoleforAWSCodeDeploy"})
  rescue Aws::IAM::Errors::EntityAlreadyExists
    #Using the existing role
  end
  eventually do  
    instance_role = @iam_client.get_role({:role_name => INSTANCE_ROLE_NAME}).role
    expect(instance_role).not_to be_nil
    @instance_role ||= instance_role.arn
  end
end

def create_instance_profile
  begin
    @instance_profile ||= @iam_client.create_instance_profile({:instance_profile_name => INSTANCE_ROLE_NAME}).instance_profile.arn
    @iam_client.add_role_to_instance_profile({:instance_profile_name => INSTANCE_ROLE_NAME, :role_name => INSTANCE_ROLE_NAME})
    eventually(:upto => 60) do
      profile = @iam_client.get_instance_profile({:instance_profile_name => INSTANCE_ROLE_NAME}).instance_profile
      expect(profile).not_to be_nil
      expect(profile.roles).not_to be_empty
      profile_arn ||= profile.arn
    end
  rescue Aws::IAM::Errors::EntityAlreadyExists
    @instance_profile ||= @iam_client.get_instance_profile({:instance_profile_name => INSTANCE_ROLE_NAME}).instance_profile.arn
  end
end

def deployment_role_policy
  "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"Service\":[\"codedeploy.amazonaws.com\"]},\"Action\":[\"sts:AssumeRole\"]}]}"
end

def instance_role_policy
  "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"Service\":[\"ec2.amazonaws.com\"]},\"Action\":[\"sts:AssumeRole\"]}]}"
end

def assert_deployment_status(expected_status, wait_sec)
  eventually(:upto => wait_sec) do
    actual_status = @codedeploy_client.get_deployment(:deployment_id => @deployment_id).deployment_info.status
    expect(actual_status).to eq(expected_status)
  end
end
