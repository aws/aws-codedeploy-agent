require 'archive/tar/minitar'
require 'zlib'
require 'pathname'
include Archive::Tar

$:.unshift File.join(File.dirname(File.expand_path('../../..', __FILE__)), 'lib')
$:.unshift File.join(File.dirname(File.expand_path('../../..', __FILE__)), 'features')
require 'aws/codedeploy/local/deployer'

LOCAL_DEPLOYMENT_GROUP_ID = 'test-local-deployments-folder'

Before("@codedeploy-local") do
  @test_directory = Dir.mktmpdir
  configure_local_agent(@test_directory)
  AwsCredentials.instance.configure
end

After("@codedeploy-local") do
  FileUtils.rm_rf(@test_directory) unless @test_directory.nil?
end

Given(/^I have a sample local (tgz|tar|zip|directory|relative_directory|custom_event_directory) bundle$/) do |bundle_type|
  case bundle_type
  when 'custom_event_directory'
    @bundle_original_directory_location = StepConstants::SAMPLE_CUSTOM_EVENT_APP_BUNDLE_FULL_PATH
  else
    @bundle_original_directory_location = StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH
  end

  expect(File.directory?(@bundle_original_directory_location)).to be true
  @bundle_type = bundle_type.include?('directory') ? 'directory' : bundle_type

  case bundle_type
  when 'directory', 'custom_event_directory'
    @bundle_location = @bundle_original_directory_location
  when 'relative_directory'
    @bundle_location = Pathname.new(@bundle_original_directory_location).relative_path_from Pathname.getwd
  when 'zip'
    @bundle_location = zip_app_bundle(@test_directory)
  when 'tar'
    @bundle_location = tar_app_bundle(@test_directory)
  when 'tgz'
    @bundle_location = tgz_app_bundle(@test_directory)
  end

  expect(File.file?(@bundle_location)).to be true unless bundle_type.include? 'directory'
end

def tar_app_bundle(temp_directory_to_create_bundle)
  tar_file_name = "#{temp_directory_to_create_bundle}/app_bundle.tar"
  old_direcory = Dir.pwd
  #Unfortunately Minitar will keep pack all the file paths as given, so unless you change directories into the location where you want to pack the files the bundle won't have the correct files and folders
  Dir.chdir @bundle_original_directory_location

  File.open(tar_file_name, 'wb') { |tar| Minitar.pack(directories_and_files_inside(@bundle_original_directory_location), tar) }

  Dir.chdir old_direcory
  tar_file_name
end

def tgz_app_bundle(temp_directory_to_create_bundle)
  tgz_file_name = "#{temp_directory_to_create_bundle}/app_bundle.tgz"
  old_direcory = Dir.pwd
  #Unfortunately Minitar will keep pack all the file paths as given, so unless you change directories into the location where you want to pack the files the bundle won't have the correct files and folders
  Dir.chdir @bundle_original_directory_location

  File.open(tgz_file_name, 'wb') do |file|
    Zlib::GzipWriter.wrap(file) do |gz|
      Minitar.pack(directories_and_files_inside(@bundle_original_directory_location), gz)
    end
  end

  Dir.chdir old_direcory
  tgz_file_name
end

When(/^I create a local deployment with my bundle with parameter (\S+)$/) do |deployment_group_id_parameter|
  @local_deployment_succeeded = create_local_deployment(nil, deployment_group_id_parameter)
end

When(/^I create a local deployment with my bundle with only events (.+)$/) do |custom_events|
  @local_deployment_succeeded = create_local_deployment(custom_events.split(' '))
end

When(/^I create a local deployment with my bundle$/) do
  @local_deployment_succeeded = create_local_deployment
end

def create_local_deployment(custom_events = nil, deployment_group_id_parameter = nil)
  if (custom_events)
    codeedeploy_command_suffix = " -e #{custom_events.join(' -e ')}"
  end

  deployment_group_id_parameter ||= '--deployment-group-id'

  system "bin/codedeploy-local --bundle-location #{@bundle_location} --type #{@bundle_type} #{deployment_group_id_parameter} #{LOCAL_DEPLOYMENT_GROUP_ID} --configuration-file #{InstanceAgent::Config.config[:config_file]}#{codeedeploy_command_suffix}"
end

Then(/^the local deployment command should succeed$/) do
  expect(@local_deployment_succeeded).to be true
end

Then(/^the expected files should have have been locally deployed to my host(| twice)$/) do |maybe_twice|
  deployment_ids = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/#{LOCAL_DEPLOYMENT_GROUP_ID}")
  step "the expected files in directory #{bundle_original_directory_location}/scripts should have have been deployed#{maybe_twice} to my host during deployment with deployment group id #{LOCAL_DEPLOYMENT_GROUP_ID} and deployment ids #{deployment_ids.join(' ')}"
end

def bundle_original_directory_location
  #Sets default value if the original location was never set, such as with an s3 uploaded bundle
  @bundle_original_directory_location ||= StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH
  @bundle_original_directory_location
end

Then(/^the scripts should have been executed during local deployment$/) do
  expected_executed_lifecycle_events = expected_executed_lifecycle_events_from_first_time_revision_is_deployed
  step "the scripts for events #{expected_executed_lifecycle_events.join(' ')} should have been executed and written to executed_proof_file in directory #{@test_directory}"
end

def expected_executed_lifecycle_events_from_first_time_revision_is_deployed
  AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS - AWS::CodeDeploy::Local::Deployer::REQUIRED_LIFECYCLE_EVENTS - %w(BeforeBlockTraffic AfterBlockTraffic ApplicationStop)
end

Then(/^the scripts should have been executed during two local deployments$/) do
  expected_executed_lifecycle_events = expected_executed_lifecycle_events_from_first_time_revision_is_deployed + (AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS - AWS::CodeDeploy::Local::Deployer::REQUIRED_LIFECYCLE_EVENTS)
  step "the scripts for events #{expected_executed_lifecycle_events.join(' ')} should have been executed and written to executed_proof_file in directory #{@test_directory}"
end

Then(/^the scripts should have been executed during local deployment with only (.+)$/) do |custom_events|
  expected_executed_lifecycle_events = custom_events.split(' ') - %w(BeforeBlockTraffic AfterBlockTraffic ApplicationStop)
  step "the scripts for events #{expected_executed_lifecycle_events.join(' ')} should have been executed and written to executed_proof_file in directory #{@test_directory}"
end
