require 'aws-sdk-core'
require 'securerandom'
require 'base64'
require 'tempfile'
require 'zip'
require 'fileutils'

$:.unshift File.join(File.dirname(File.expand_path('..', __FILE__)), 'lib')
$:.unshift File.join(File.dirname(File.expand_path('..', __FILE__)), 'features')
require 'aws_credentials'
require 'instance_agent'
require 'instance_agent/plugins/codedeploy/register_plugin'
require 'instance_agent/config'
require 'instance_agent/log'
require 'instance_agent/runner/master'
require 'instance_agent/platform'
require 'instance_agent/platform/linux_util'
require 'aws/codedeploy/local/deployer'

IS_WINDOWS = (RbConfig::CONFIG['host_os'] =~ /mswin|mingw|cygwin/)
CODEDEPLOY_TEST_PREFIX = "codedeploy-agent-integ-test-"
DEPLOYMENT_ROLE_NAME = "#{CODEDEPLOY_TEST_PREFIX}deployment-role"
APP_BUNDLE_BUCKET_SUFFIX = IS_WINDOWS ? '-windows' : '-linux'
APP_BUNDLE_BUCKET = "#{CODEDEPLOY_TEST_PREFIX}bucket#{APP_BUNDLE_BUCKET_SUFFIX}"
APP_BUNDLE_KEY = 'app_bundle.zip'
REGION = 'us-west-2'
SAMPLE_APP_BUNDLE_DIRECTORY = IS_WINDOWS ? 'sample_app_bundle_windows' : 'sample_app_bundle_linux'

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
  configure_local_agent
  AwsCredentials.instance.configure

  #instantiate these clients first so they use user's aws creds instead of assumed role creds
  @codedeploy_client = Aws::CodeDeploy::Client.new
  @iam_client = Aws::IAM::Client.new
end

def configure_local_agent
  @working_directory = Dir.mktmpdir
  puts "Running test out of temp directory #{@working_directory}"
  ProcessManager::Config.init
  InstanceAgent::Log.init(File.join(@working_directory, 'codedeploy-agent.log'))
  InstanceAgent::Config.init
  InstanceAgent::Platform.util = IS_WINDOWS ? InstanceAgent::WindowsUtil : InstanceAgent::LinuxUtil

  if IS_WINDOWS then configure_windows_certificate end

  configuration_contents = <<-CONFIG
---
:log_aws_wire: false
:log_dir: #{@working_directory}
:pid_dir: #{@working_directory}
:program_name: codedeploy-agent
:root_dir: #{@working_directory}/deployment-root
:verbose: true
:wait_between_runs: 1
:proxy_uri:
:max_revisions: 5
  CONFIG

  InstanceAgent::Config.config[:config_file] = "#{@working_directory}/codedeployagent.yml"
  File.open(InstanceAgent::Config.config[:config_file], 'w') { |file| file.write(configuration_contents) }

  InstanceAgent::Config.load_config
end

def configure_windows_certificate
  cert_dir = File.expand_path(File.join(File.dirname(__FILE__), '..\certs'))
  Aws.config[:ssl_ca_bundle] = File.join(cert_dir, 'windows-ca-bundle.crt')
  ENV['AWS_SSL_CA_DIRECTORY'] = File.join(cert_dir, 'windows-ca-bundle.crt')
  ENV['SSL_CERT_FILE'] = File.join(cert_dir, 'windows-ca-bundle.crt')
end

After("@codedeploy-agent") do
  @thread.kill
  @codedeploy_client.delete_application({:application_name => @application_name}) unless @application_name.nil?
  @codedeploy_client.deregister_on_premises_instance({:instance_name => instance_name})
  FileUtils.rm_rf(@working_directory)
end

Given(/^I have a CodeDeploy application$/) do
  @application_name = "codedeploy-integ-testapp-#{SecureRandom.hex(10)}"
  @codedeploy_client.create_application(:application_name => @application_name)
end

Given(/^I register my host in CodeDeploy$/) do
  iam_session_arn = create_iam_assume_role_session
  @codedeploy_client.register_on_premises_instance(:instance_name => instance_name, :iam_session_arn => iam_session_arn)
  @codedeploy_client.add_tags_to_on_premises_instances(:instance_names => [instance_name], :tags => [{:key => instance_name}])

  configure_agent_for_on_premise(iam_session_arn)
end

def configure_agent_for_on_premise(iam_session_arn)
  on_premise_configuration_content = <<-CONFIG
---
region: #{REGION}
aws_credentials_file: #{create_aws_credentials_file}
iam_session_arn: #{iam_session_arn}
  CONFIG

  InstanceAgent::Config.config[:on_premises_config_file] = "#{@working_directory}/codedeploy.onpremises.yml"
  File.open(InstanceAgent::Config.config[:on_premises_config_file], 'w') { |file| file.write(on_premise_configuration_content) }
  InstanceAgent::Plugins::CodeDeployPlugin::OnPremisesConfig.configure
end

def create_aws_credentials_file
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

Given(/^I have a sample bundle uploaded to s3$/) do
  s3 = Aws::S3::Client.new

  begin
    s3.create_bucket({
      bucket: APP_BUNDLE_BUCKET, # required
      create_bucket_configuration: {
        location_constraint: REGION,
      }
    })
  rescue Aws::S3::Errors::BucketAlreadyOwnedByYou
    #Already created the bucket
  end

  File.open(zip_app_bundle, 'rb') do |file|
    s3.put_object(bucket: APP_BUNDLE_BUCKET, key: APP_BUNDLE_KEY, body: file)
  end
end

def zip_app_bundle
  zip_file_name = "#{@working_directory}/#{APP_BUNDLE_KEY}"
  zip_directory("#{Dir.pwd}/features/resources/#{SAMPLE_APP_BUNDLE_DIRECTORY}", zip_file_name)
  zip_file_name
end

def zip_directory(input_dir, output_file)
  entries = directories_and_files_inside(input_dir)
  zip_io = Zip::File.open(output_file, Zip::File::CREATE)

  write_zip_entries(entries, '', input_dir, zip_io)
  zip_io.close()
end

def write_zip_entries(entries, path, input_dir, zip_io)
  entries.each do |entry|
    zipFilePath = path == "" ? entry : File.join(path, entry)
    diskFilePath = File.join(input_dir, zipFilePath)
    if File.directory?(diskFilePath)
      zip_io.mkdir(zipFilePath)
      folder_entries = directories_and_files_inside(diskFilePath)
      write_zip_entries(folder_entries, zipFilePath, input_dir, zip_io)
    else
      zip_io.get_output_stream(zipFilePath){ |f| f.write(File.open(diskFilePath, "rb").read())}
    end
  end
end

def directories_and_files_inside(dir)
  Dir.entries(dir) - %w(.. .)
end

When(/^I create a deployment for the application and deployment group with the test S(\d+) revision$/) do |arg1|
  @deployment_id = @codedeploy_client.create_deployment({:application_name => @application_name,
                            :deployment_group_name => @deployment_group_name,
                            :revision => { :revision_type => "S3",
                                           :s3_location => {
                                             :bucket => APP_BUNDLE_BUCKET,
                                             :key => APP_BUNDLE_KEY,
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

Then(/^the expected files should have have been deployed to my host$/) do
  directories_in_deployment_root_folder = directories_and_files_inside(InstanceAgent::Config.config[:root_dir])
  expect(directories_in_deployment_root_folder.size).to eq(3)

  deployment_group_id = @codedeploy_client.get_deployment_group({
    application_name: @application_name,
    deployment_group_name: @deployment_group_name,
  }).deployment_group_info.deployment_group_id

  #ordering of the directories depends on the deployment group id, so using include instead of eq
  expect(directories_in_deployment_root_folder).to include(*%W(deployment-instructions deployment-logs #{deployment_group_id}))

  files_in_deployment_logs_folder = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/deployment-logs")
  expect(files_in_deployment_logs_folder.size).to eq(1)
  expect(files_in_deployment_logs_folder).to eq(%w(codedeploy-agent-deployments.log))

  directories_in_deployment_group_id_folder = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/#{deployment_group_id}")
  expect(directories_in_deployment_group_id_folder.size).to eq(1)
  expect(directories_in_deployment_group_id_folder).to eq([@deployment_id])

  files_and_directories_in_deployment_id_folder = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/#{deployment_group_id}/#{@deployment_id}")
  expect(files_and_directories_in_deployment_id_folder.size).to eq(3)
  expect(files_and_directories_in_deployment_id_folder).to include(*%w(bundle.tar logs deployment-archive))

  files_and_directories_in_deployment_archive_folder = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/#{deployment_group_id}/#{@deployment_id}/deployment-archive")
  expect(files_and_directories_in_deployment_archive_folder.size).to eq(2)
  expect(files_and_directories_in_deployment_archive_folder).to include(*%w(appspec.yml scripts))

  files_in_scripts_folder = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/#{deployment_group_id}/#{@deployment_id}/deployment-archive/scripts")
  expect(files_in_scripts_folder.size).to eq(9)

  sample_app_bundle_script_files = directories_and_files_inside("#{Dir.pwd}/features/resources/#{SAMPLE_APP_BUNDLE_DIRECTORY}/scripts")
  expect(files_in_scripts_folder).to eq(sample_app_bundle_script_files)
end

Then(/^the scripts should have been executed$/) do
  #Scripts contain echo '<LifecycleEventName>' >> ../../../../../executed_proof_file
  #This means it appends to executed_proof_file in our test working directory so we
  #can check that for proof the files actually were executed
  #
  #Interestingly I discovered that these are the only executed steps. So codedeploy does not run ApplicationStop for example if there's no previous revision.
  expected_executed_lifecycle_events = %w(BeforeInstall AfterInstall ApplicationStart ValidateService)
  file_lines = File.read("#{@working_directory}/executed_proof_file").split("\n")
  expect(file_lines.size).to eq(expected_executed_lifecycle_events.size)
  expect(file_lines).to eq(expected_executed_lifecycle_events)
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

def create_iam_assume_role_session
  sts = Aws::STS::Client.new
  assume_role_response = sts.assume_role({
    duration_seconds: 3600,
    role_arn: 'arn:aws:iam::910828814654:role/CodeDeployOnPremInstanceRole',
    role_session_name: instance_name,
  })

  @iam_session_access_key_id = assume_role_response.to_h[:credentials][:access_key_id]
  @iam_session_secret_access_key = assume_role_response.to_h[:credentials][:secret_access_key]
  @iam_session_token = assume_role_response.to_h[:credentials][:session_token]

  assume_role_response.to_h[:assumed_role_user][:arn]
end

def deployment_role_policy
  "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"Service\":[\"codedeploy.amazonaws.com\"]},\"Action\":[\"sts:AssumeRole\"]}]}"
end

def assert_deployment_status(expected_status, wait_sec)
  eventually(:upto => wait_sec) do
    actual_status = @codedeploy_client.get_deployment(:deployment_id => @deployment_id).deployment_info.status
    expect(actual_status).to eq(expected_status)
  end
end
