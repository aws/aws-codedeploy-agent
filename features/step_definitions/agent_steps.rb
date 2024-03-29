require 'securerandom'
require 'base64'
require 'tempfile'
require 'zip'
require 'fileutils'
require 'aws-sdk-core'
require 'aws-sdk-codedeploy'
require 'aws-sdk-iam'

$:.unshift File.join(File.dirname(File.expand_path('../..', __FILE__)), 'lib')
$:.unshift File.join(File.dirname(File.expand_path('../..', __FILE__)), 'features')
require 'instance_agent'
require 'instance_agent/plugins/codedeploy/register_plugin'
require 'instance_agent/config'
require 'instance_agent/log'
require 'instance_agent/runner/master'
require 'instance_agent/platform'
require 'instance_agent/platform/linux_util'
require 'aws/codedeploy/local/deployer'
require 'step_definitions/step_constants'

DEPLOYMENT_ROLE_NAME = "#{StepConstants::CODEDEPLOY_TEST_PREFIX}deployment-role"
INSTANCE_ROLE_NAME = "#{StepConstants::CODEDEPLOY_TEST_PREFIX}instance-role"
INSTANCE_USER_NAME = "#{StepConstants::CODEDEPLOY_TEST_PREFIX}instance-user"
DEPLOYMENT_ROLE_POLICY = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"Service\":[\"codedeploy.amazonaws.com\"]},\"Action\":[\"sts:AssumeRole\"]}]}"
S3_READONLY_ACCESS_ARN = "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess"
CODEDEPLOY_ROLE_ARN = "arn:aws:iam::aws:policy/service-role/AWSCodeDeployRole"
ISOLATED_ENV_KEYS = %w(AWS_REGION AWS_ACCESS_KEY AWS_SECRET_KEY AWS_HOST_IDENTIFIER AWS_CREDENTIALS_FILE
                       AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_CREDENTIAL_FILE)

def instance_name
  @instance_name ||= SecureRandom.uuid
  @instance_name
end

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
  @working_directory = Dir.mktmpdir
  configure_local_agent(@working_directory)

  #instantiate these clients first so they use user's aws creds instead of assumed role creds
  @codedeploy_client = Aws::CodeDeploy::Client.new
  @iam_client = Aws::IAM::Client.new
  @sts = Aws::STS::Client.new
end

def configure_local_agent(working_directory)
  ProcessManager::Config.init
  InstanceAgent::Log.init(File.join(working_directory, 'codedeploy-agent.log'))
  InstanceAgent::Config.init
  InstanceAgent::Platform.util = StepConstants::IS_WINDOWS ? InstanceAgent::WindowsUtil : InstanceAgent::LinuxUtil
  InstanceAgent::Config.config[:on_premises_config_file] = "#{working_directory}/codedeploy.onpremises.yml"

  configuration_contents = <<-CONFIG
---
:log_aws_wire: false
:log_dir: #{working_directory}
:pid_dir: #{working_directory}
:program_name: codedeploy-agent
:root_dir: #{working_directory}/deployment-root
:on_premises_config_file: #{InstanceAgent::Config.config[:on_premises_config_file]}
:verbose: true
:wait_between_runs: 1
:proxy_uri:
:max_revisions: 5
  CONFIG

  InstanceAgent::Config.config[:config_file] = "#{working_directory}/codedeployagent.yml"
  File.open(InstanceAgent::Config.config[:config_file], 'w') { |file| file.write(configuration_contents) }

  InstanceAgent::Config.load_config
end

After("@codedeploy-agent") do
  @thread.kill unless @thread.nil?
  @codedeploy_client.delete_application({:application_name => @application_name}) unless @application_name.nil?
  @codedeploy_client.deregister_on_premises_instance({:instance_name => instance_name})
  FileUtils.rm_rf(@working_directory) unless @working_directory.nil?
end

Before("@isolate-agent-config") do
  # If not reset DeploymentLog, log may leak to the log file of the last scenario
  class InstanceAgent::DeploymentLog
    def self.reset
      Singleton.send :__init__, self
    end
  end
  InstanceAgent::DeploymentLog.reset
  # agent may override config like Aws.config[:credentials] and ENV['AWS_ACCESS_KEY']. We don't want to leak agent's config to test runner.
  @aws_config = Aws.config
  Aws.config = Aws.config.clone
  @env_vars = {}
  ISOLATED_ENV_KEYS.each do |key|
    @env_vars[key] = ENV[key]
  end
end

After("@isolate-agent-config") do
  Aws.config = @aws_config
  ISOLATED_ENV_KEYS.each do |key|
    ENV[key] = @env_vars[key]
  end
end

Given(/^I have a CodeDeploy application$/) do
  @application_name = "codedeploy-integ-testapp-#{SecureRandom.hex(10)}"
  @codedeploy_client.create_application(:application_name => @application_name)
end

Given(/^I register my host in CodeDeploy using (IAM user|IAM session)$/) do |method|
  case method
  when "IAM user"
    register_on_premises_with_iam_user
  when "IAM session"
    register_on_premises_with_iam_session
  end
end

def register_on_premises_with_iam_user
  user = create_instance_user
  access_key = recreate_instance_user_access_key
  begin
    @codedeploy_client.register_on_premises_instance(:instance_name => instance_name, :iam_user_arn => user.arn)
  rescue Aws::CodeDeploy::Errors::IamUserArnAlreadyRegisteredException
    # One IAM user cannot be used used to register more than one instance. Clean up old instances.
    next_token = nil
    loop do
      resp = @codedeploy_client.list_on_premises_instances({:registration_status => "Registered", :next_token => next_token})
      instance_names = resp.instance_names
      next_token = resp.next_token
      @codedeploy_client.batch_get_on_premises_instances({:instance_names => instance_names}).instance_infos.select do | instance |
        instance.iam_user_arn == user.arn
      end.each do | instance |
        @codedeploy_client.deregister_on_premises_instance({:instance_name => instance.instance_name})
        next_token = nil
      end
      break if next_token.nil?
    end
    @codedeploy_client.register_on_premises_instance(:instance_name => instance_name, :iam_user_arn => user.arn)
  end
  @codedeploy_client.add_tags_to_on_premises_instances(:instance_names => [instance_name], :tags => [{:key => instance_name}])
  configure_agent_for_on_premise({:iam_user_arn => user.arn,
                                  :access_key_id => access_key.access_key_id,
                                  :secret_access_key => access_key.secret_access_key})
end

def register_on_premises_with_iam_session
  iam_session_arn = create_iam_assume_role_session
  @codedeploy_client.register_on_premises_instance(:instance_name => instance_name, :iam_session_arn => iam_session_arn)
  @codedeploy_client.add_tags_to_on_premises_instances(:instance_names => [instance_name], :tags => [{:key => instance_name}])
  configure_agent_for_on_premise({:iam_session_arn => iam_session_arn})
end

def configure_agent_for_on_premise(options={})
  if !options[:iam_session_arn] == !options[:iam_user_arn]
    raise "Exactly one of :iam_session_arn and :iam_user_arn is required"
  end
  if options[:iam_session_arn]
    on_premise_configuration_content = <<-CONFIG
---
region: #{Aws.config[:region]}
aws_credentials_file: #{create_aws_credentials_session_file}
iam_session_arn: #{options[:iam_session_arn]}
  CONFIG
  else
    on_premise_configuration_content = <<-CONFIG
---
region: #{Aws.config[:region]}
aws_access_key_id: #{options[:access_key_id]}
aws_secret_access_key: #{options[:secret_access_key]}
iam_user_arn: #{options[:iam_user_arn]}
    CONFIG
  end
  File.open(InstanceAgent::Config.config[:on_premises_config_file], 'w') { |file| file.write(on_premise_configuration_content) }
  InstanceAgent::Plugins::CodeDeployPlugin::OnPremisesConfig.configure
end

def create_aws_credentials_session_file
  aws_credentials_content = <<-CREDENTIALS
---
[default]
aws_access_key_id=#{@iam_session_access_key_id}
aws_secret_access_key=#{@iam_session_secret_access_key}
aws_session_token=#{@iam_session_token}
  CREDENTIALS

  aws_credentials_file_location = "#{@working_directory}/credentials"
  File.open(aws_credentials_file_location, 'w') { |file| file.write(aws_credentials_content) }
  aws_credentials_file_location
end

Given(/^I startup the CodeDeploy agent locally$/) do
  @thread = Thread.start do
    loop do
      InstanceAgent::Plugins::CodeDeployPlugin::CommandPoller.runner.run
      sleep InstanceAgent::Config.config[:wait_between_runs].to_i
    end
  end
end

Given(/^I have a deployment group containing my host$/) do
  @deployment_group_name = "codedeploy-integ-testdg-#{SecureRandom.hex(10)}"
  create_deployment_role
  create_deployment_group
end

When(/^I create a deployment for the application and deployment group with the test S3 revision$/) do
  @deployment_id = @codedeploy_client.create_deployment({:application_name => @application_name,
                            :deployment_group_name => @deployment_group_name,
                            :revision => { :revision_type => "S3",
                                           :s3_location => {
                                             :bucket => StepConstants::APP_BUNDLE_BUCKET,
                                             :key => StepConstants::APP_BUNDLE_KEY,
                                             :bundle_type => "zip"
                                           }
                                         },
                             :deployment_config_name => "CodeDeployDefault.OneAtATime",
                             :description => "CodeDeploy agent integration test",
                          }).deployment_id
end

Then(/^the overall deployment should eventually be in progress$/) do
  assert_deployment_status("InProgress", 30)
end

Then(/^the deployment should contain all the instances I tagged$/) do
  instances = @codedeploy_client.list_deployment_instances(:deployment_id => @deployment_id).instances_list
  expect(instances.size).to eq(1)
  expect(instances.first).to eq(instance_name)
end

Then(/^the overall deployment should eventually succeed$/) do
  assert_deployment_status("Succeeded", 60)
end

Then(/^the expected files \((\d+)\) should have have been deployed to my host$/) do |expected_file_count|
  deployment_group_id = @codedeploy_client.get_deployment_group({
    application_name: @application_name,
    deployment_group_name: @deployment_group_name,
  }).deployment_group_info.deployment_group_id

  step "the expected files (#{expected_file_count}) in directory #{Dir.pwd}/features/resources/#{StepConstants::SAMPLE_APP_BUNDLE_DIRECTORY}/scripts should have have been deployed to my host during deployment with deployment group id #{deployment_group_id} and deployment ids #{@deployment_id}"
end

Then(/^the scripts should have been executed$/) do
  #Scripts contain echo '<LifecycleEventName>' >> ../../../../../executed_proof_file
  #This means it appends to executed_proof_file in our test working directory so we
  #can check that for proof the files actually were executed
  #
  #Interestingly I discovered that these are the only executed steps. So codedeploy does not run ApplicationStop for example if there's no previous revision.
  expected_executed_lifecycle_events = %w(BeforeInstall AfterInstall ApplicationStart ValidateService)
  step "the scripts for events #{expected_executed_lifecycle_events.join(' ')} should have been executed and written to executed_proof_file in directory #{@working_directory}"
end

def create_deployment_group
  @codedeploy_client.create_deployment_group({:application_name => @application_name,
                                  :deployment_group_name => @deployment_group_name,
                                  :on_premises_instance_tag_filters => [{:key => instance_name, type: 'KEY_ONLY'}],
                                  :service_role_arn => @deployment_role})
end

def create_deployment_role
  begin
    @iam_client.create_role({:role_name => DEPLOYMENT_ROLE_NAME,
                             :assume_role_policy_document => DEPLOYMENT_ROLE_POLICY}).role.arn
    @iam_client.attach_role_policy({:role_name => DEPLOYMENT_ROLE_NAME,
                                    :policy_arn => CODEDEPLOY_ROLE_ARN})
  rescue Aws::IAM::Errors::EntityAlreadyExists
    #Using the existing role
  end
  eventually(:upto => 60) do
    deployment_role = @iam_client.get_role({:role_name => DEPLOYMENT_ROLE_NAME}).role
    expect(deployment_role).not_to be_nil
    @deployment_role ||= deployment_role.arn
  end
end

def create_instance_user
  begin
    user = @iam_client.create_user({:user_name => INSTANCE_USER_NAME}).user
    @iam_client.wait_until(:user_exists, user_name: INSTANCE_USER_NAME)
    @iam_client.attach_user_policy({:user_name => INSTANCE_USER_NAME,
                                    :policy_arn => S3_READONLY_ACCESS_ARN})
  rescue Aws::IAM::Errors::EntityAlreadyExists
    user = @iam_client.get_user({:user_name => INSTANCE_USER_NAME}).user
  end
  user
end

def recreate_instance_user_access_key
  @iam_client.list_access_keys({:user_name => INSTANCE_USER_NAME}).access_key_metadata.each do |key|
    @iam_client.delete_access_key({:user_name => key.user_name,
                                    :access_key_id => key.access_key_id})
  end
  @iam_client.create_access_key({:user_name => INSTANCE_USER_NAME}).access_key
end

def create_instance_role
  begin
    @iam_client.create_role({:role_name => INSTANCE_ROLE_NAME,
                             :assume_role_policy_document => instance_role_policy}).role.arn
    @iam_client.attach_role_policy({:role_name => INSTANCE_ROLE_NAME,
                                    :policy_arn => S3_READONLY_ACCESS_ARN})
  rescue Aws::IAM::Errors::EntityAlreadyExists
    #Using the existing role
  end

  instance_role_arn = eventually do
    instance_role = @iam_client.get_role({:role_name => INSTANCE_ROLE_NAME}).role
    expect(instance_role).not_to be_nil
    expect(instance_role.assume_role_policy_document).not_to be_nil
    instance_role.arn
  end

  @iam_client.update_assume_role_policy({
    policy_document: instance_role_policy,
    role_name: INSTANCE_ROLE_NAME,
  })

  instance_role_arn
end

def instance_role_policy
  "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"\",\"Effect\":\"Allow\",\"Principal\":{\"Service\":[\"codedeploy.amazonaws.com\"]},\"Action\":[\"sts:AssumeRole\"]},{\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"#{current_user_or_role_arn}\"},\"Action\":[\"sts:AssumeRole\"]}]}"
end

def current_user_or_role_arn
  @sts.get_caller_identity.arn
end

def create_iam_assume_role_session
  # The assume role policy takes some time to propagate so wrapping assume role call in eventually clause
  assume_role_response = eventually do
    @sts.assume_role({
      duration_seconds: 3600,
      role_arn: create_instance_role,
      role_session_name: instance_name,
    })
  end

  @iam_session_access_key_id = assume_role_response.to_h[:credentials][:access_key_id]
  @iam_session_secret_access_key = assume_role_response.to_h[:credentials][:secret_access_key]
  @iam_session_token = assume_role_response.to_h[:credentials][:session_token]

  assume_role_response.to_h[:assumed_role_user][:arn]
end

def assert_deployment_status(expected_status, wait_sec)
  eventually(:upto => wait_sec) do
    actual_status = @codedeploy_client.get_deployment(:deployment_id => @deployment_id).deployment_info.status
    expect(actual_status).to eq(expected_status)
  end
end
