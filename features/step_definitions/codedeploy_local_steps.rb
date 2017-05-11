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

Given(/^I have a sample local (tgz|tar|zip|directory|relative_directory) bundle$/) do |bundle_type|
  expect(File.directory?(StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH)).to be true
  @bundle_type = bundle_type

  case bundle_type
  when 'directory'
    @bundle_location = StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH
  when 'relative_directory'
    @bundle_location = Pathname.new(StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH).relative_path_from Pathname.getwd
    @bundle_type = 'directory'
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
  Dir.chdir StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH

  File.open(tar_file_name, 'wb') { |tar| Minitar.pack(directories_and_files_inside(StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH), tar) }

  Dir.chdir old_direcory
  tar_file_name
end

def tgz_app_bundle(temp_directory_to_create_bundle)
  tgz_file_name = "#{temp_directory_to_create_bundle}/app_bundle.tgz"
  old_direcory = Dir.pwd
  #Unfortunately Minitar will keep pack all the file paths as given, so unless you change directories into the location where you want to pack the files the bundle won't have the correct files and folders
  Dir.chdir StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH

  File.open(tgz_file_name, 'wb') do |file|
    Zlib::GzipWriter.wrap(file) do |gz|
      Minitar.pack(directories_and_files_inside(StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH), gz)
    end
  end

  Dir.chdir old_direcory
  tgz_file_name
end

When(/^I create a local deployment with my bundle$/) do
  @local_deployment_succeeded = system "bin/codedeploy-local --bundle-location #{@bundle_location} --type #{@bundle_type} --deployment-group-id #{LOCAL_DEPLOYMENT_GROUP_ID} --configuration-file #{InstanceAgent::Config.config[:config_file]}"
end

Then(/^the local deployment command should succeed$/) do
  expect(@local_deployment_succeeded).to be true
end

Then(/^the expected files should have have been locally deployed to my host$/) do
  deployment_id = most_recent_directory_or_file("#{InstanceAgent::Config.config[:root_dir]}/#{LOCAL_DEPLOYMENT_GROUP_ID}")
  step "the expected files should have have been deployed to my host during deployment with deployment group id #{LOCAL_DEPLOYMENT_GROUP_ID} and deployment id #{deployment_id}"
end

def most_recent_directory_or_file(directory)
  File.basename Dir.glob("#{directory}/*").max_by {|f| File.mtime(f)}
end

Then(/^the scripts should have been executed during local deployment$/) do
  # We need to remove lifecycle events that act on the previous revision since there's no previous revision they're alwyays skipped as part of these tests (TODO: create a test which runs 2 local deployments and verifies that those events get run)
  expected_executed_lifecycle_events = AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS - AWS::CodeDeploy::Local::Deployer::REQUIRED_LIFECYCLE_EVENTS - %w(BeforeBlockTraffic AfterBlockTraffic ApplicationStop)
  step "the scripts for events #{expected_executed_lifecycle_events.join(' ')} should have been executed and written to executed_proof_file in directory #{@test_directory}"
end
