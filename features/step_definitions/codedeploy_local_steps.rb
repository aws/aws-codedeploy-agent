require 'archive/tar/minitar'
require 'zlib'
require 'pathname'
include Archive::Tar

$:.unshift File.join(File.dirname(File.expand_path('../../..', __FILE__)), 'lib')
$:.unshift File.join(File.dirname(File.expand_path('../../..', __FILE__)), 'features')
require 'aws/codedeploy/local/deployer'

LOCAL_DEPLOYMENT_GROUP_ID = 'test-local-deployments-folder'
FILE_TO_POTENTIALLY_OVERWRITE = 'file-to-potentially-overwrite'
BUNDLE_FILE_CONTENT_TO_POTENTIALLY_OVERWRITE = 'BUNDLE_FILE_CONTENT_TO_POTENTIALLY_OVERWRITE'
ORIGINAL_FILE_CONTENT_TO_POTENTIALLY_OVERWRITE = 'ORIGINAL_FILE_CONTENT_TO_POTENTIALLY_OVERWRITE'

Before("@codedeploy-local") do
  @test_directory = Dir.mktmpdir
  configure_local_agent(@test_directory)
end

After("@codedeploy-local") do
  FileUtils.rm_rf(@test_directory) unless @test_directory.nil?
end

Given(/^I have a sample local (tgz|tar|zip|directory|relative_directory|custom_event_directory|directory_with_destination_files) bundle$/) do |bundle_type|
  case bundle_type
  when 'custom_event_directory'
    @bundle_original_directory_location = StepConstants::SAMPLE_CUSTOM_EVENT_APP_BUNDLE_FULL_PATH
  when 'directory_with_destination_files'
    @bundle_original_directory_location = create_bundle_with_appspec_containing_source_and_destination_file(StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH)
  else
    @bundle_original_directory_location = StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH
  end

  expect(File.directory?(@bundle_original_directory_location)).to be true
  @bundle_type = bundle_type.include?('directory') ? 'directory' : bundle_type

  case bundle_type
  when 'relative_directory'
    @bundle_location = Pathname.new(@bundle_original_directory_location).relative_path_from Pathname.getwd
  when 'zip'
    @bundle_location = zip_app_bundle(@test_directory)
  when 'tar'
    @bundle_location = tar_app_bundle(@test_directory)
  when 'tgz'
    @bundle_location = tgz_app_bundle(@test_directory)
  else
    @bundle_location = @bundle_original_directory_location
  end

  expect(File.file?(@bundle_location)).to be true unless bundle_type.include? 'directory'
end

def tar_app_bundle(temp_directory_to_create_bundle)
  tar_file_name = "#{temp_directory_to_create_bundle}/app_bundle.tar"
  old_direcory = Dir.pwd
  #Unfortunately Minitar will keep pack all the file paths as given, so unless you change directories into the location where you want to pack the files the bundle won't have the correct files and folders
  Dir.chdir @bundle_original_directory_location

  File.open(tar_file_name, 'wb') { |tar| Minitar.pack(InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.directories_and_files_inside(@bundle_original_directory_location), tar) }

  Dir.chdir old_direcory
  tar_file_name
end

def create_bundle_with_appspec_containing_source_and_destination_file(source_bundle_location)
  bundle_final_location = "#{@test_directory}/bundle_with_appspec_containing_source_and_destination_file"
  FileUtils.cp_r source_bundle_location, bundle_final_location
  # Remove the appspec file since we're going to overwrite it with a new one
  FileUtils.rm %W(#{bundle_final_location}/appspec.yml)
  # Read the default appspec.yml file
  File.open("#{source_bundle_location}/appspec.yml", 'r') do |old_appspec|
    File.open("#{bundle_final_location}/appspec.yml", 'w') do |new_appspec|
      # Create the new appspec in our bundle location but add the source and destination file lines
      old_appspec.each do |line|
        new_appspec << line
        if line.start_with?('os:')
          new_appspec.puts 'files:'
          new_appspec.puts "  - source: #{FILE_TO_POTENTIALLY_OVERWRITE}"
          new_appspec.puts "    destination: #{@test_directory}"
        end
      end
    end
  end
  bundle_final_location
end

Given(/^I have existing file in destination$/) do
  # Create the file to be copied in both our bundle and our destination for testing file overwrite behavior
  File.open("#{@bundle_location}/#{FILE_TO_POTENTIALLY_OVERWRITE}", 'w') {|f| f.write(BUNDLE_FILE_CONTENT_TO_POTENTIALLY_OVERWRITE)}
  File.open("#{@test_directory}/#{FILE_TO_POTENTIALLY_OVERWRITE}", 'w') {|f| f.write(ORIGINAL_FILE_CONTENT_TO_POTENTIALLY_OVERWRITE)}
end

def tgz_app_bundle(temp_directory_to_create_bundle)
  tgz_file_name = "#{temp_directory_to_create_bundle}/app_bundle.tgz"
  old_direcory = Dir.pwd
  #Unfortunately Minitar will keep pack all the file paths as given, so unless you change directories into the location where you want to pack the files the bundle won't have the correct files and folders
  Dir.chdir @bundle_original_directory_location

  File.open(tgz_file_name, 'wb') do |file|
    Zlib::GzipWriter.wrap(file) do |gz|
      Minitar.pack(InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.directories_and_files_inside(@bundle_original_directory_location), gz)
    end
  end

  Dir.chdir old_direcory
  tgz_file_name
end

When(/^I create a local deployment with my bundle with only events (.+)$/) do |custom_events|
  @local_deployment_succeeded = create_local_deployment(custom_events.split(' '))
end

When(/^I create a local deployment with my bundle with file-exists-behavior (DISALLOW|OVERWRITE|RETAIN|MISSING)$/) do |file_exists_behavior|
  case file_exists_behavior
  when 'MISSING'
    @local_deployment_succeeded = create_local_deployment
  else
    @local_deployment_succeeded = create_local_deployment(nil, file_exists_behavior)
  end
end

When(/^I create a local deployment with my bundle$/) do
  @local_deployment_succeeded = create_local_deployment
end

def create_local_deployment(custom_events = nil, file_exists_behavior = nil)
  if (custom_events)
    codeedeploy_command_suffix = " --events #{custom_events.join(',')}"
  elsif (file_exists_behavior)
    codeedeploy_command_suffix = " --file-exists-behavior #{file_exists_behavior}"
  end

  # Windows doesn't respect shebang lines so ruby needs to be specified
  ruby_prefix_for_windows = StepConstants::IS_WINDOWS ? "ruby " : ""

  system "#{ruby_prefix_for_windows}bin/codedeploy-local --bundle-location #{@bundle_location} --type #{@bundle_type} --deployment-group #{LOCAL_DEPLOYMENT_GROUP_ID} --agent-configuration-file #{InstanceAgent::Config.config[:config_file]}#{codeedeploy_command_suffix}"
end

Then(/^the local deployment command should succeed$/) do
  expect(@local_deployment_succeeded).to be true
end

Then(/^the local deployment command should fail$/) do
  expect(@local_deployment_succeeded).to be false
end

Then(/^the expected files should have have been locally deployed to my host(| twice)$/) do |maybe_twice|
  deployment_ids = InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/#{LOCAL_DEPLOYMENT_GROUP_ID}")
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

Then(/^the expected existing file should end up like file-exists-behavior (OVERWRITE|RETAIN) specifies$/) do |file_exists_behavior|
  file_to_potentially_overwrite_contents = IO.read("#{@test_directory}/#{FILE_TO_POTENTIALLY_OVERWRITE}")

  case file_exists_behavior
  when 'OVERWRITE'
    expect(file_to_potentially_overwrite_contents).to eq(BUNDLE_FILE_CONTENT_TO_POTENTIALLY_OVERWRITE)
  when 'RETAIN'
    expect(file_to_potentially_overwrite_contents).to eq(ORIGINAL_FILE_CONTENT_TO_POTENTIALLY_OVERWRITE)
  end
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
