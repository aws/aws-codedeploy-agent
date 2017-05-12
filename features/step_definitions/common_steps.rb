require 'aws-sdk-core'

$:.unshift File.join(File.dirname(File.expand_path('../..', __FILE__)), 'features')
require 'step_definitions/step_constants'

Given(/^I have a sample bundle uploaded to s3$/) do
  #Passing in credentials intentionally because otherwise
  #it uses assume role credentials
  credentials = Aws::Credentials.new(
    ENV['AWS_ACCESS_KEY_ID'],
    ENV['AWS_SECRET_ACCESS_KEY'],
    ENV['AWS_SESSION_TOKEN']
  )

  s3 = Aws::S3::Client.new(
    region: ENV['AWS_REGION'],
    credentials: credentials
  )

  begin
    s3.create_bucket({
      bucket: StepConstants::APP_BUNDLE_BUCKET, # required
      create_bucket_configuration: {
        location_constraint: StepConstants::REGION,
      }
    })
  rescue Aws::S3::Errors::BucketAlreadyOwnedByYou
    #Already created the bucket
  end

  Dir.mktmpdir do |temp_directory_to_create_zip_file|
    File.open(zip_app_bundle(temp_directory_to_create_zip_file), 'rb') do |file|
      s3.put_object(bucket: StepConstants::APP_BUNDLE_BUCKET, key: StepConstants::APP_BUNDLE_KEY, body: file)
    end
  end

  @bundle_type = 'zip'
  @bundle_location = "s3://#{StepConstants::APP_BUNDLE_BUCKET}/#{StepConstants::APP_BUNDLE_KEY}"
end

def zip_app_bundle(temp_directory_to_create_bundle)
  zip_file_name = "#{temp_directory_to_create_bundle}/#{StepConstants::APP_BUNDLE_KEY}"
  zip_directory(StepConstants::SAMPLE_APP_BUNDLE_FULL_PATH, zip_file_name)
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

def directories_and_files_inside(directory)
  Dir.entries(directory) - %w(.. .)
end

Then(/^the expected files in directory (\S+) should have have been deployed to my host during deployment with deployment group id (\S+) and deployment id (\S+)$/) do |expected_scripts_directory, deployment_group_id, deployment_id|
  directories_in_deployment_root_folder = directories_and_files_inside(InstanceAgent::Config.config[:root_dir])
  expect(directories_in_deployment_root_folder.size).to eq(3)

  #ordering of the directories depends on the deployment group id, so using include instead of eq
  expect(directories_in_deployment_root_folder).to include(*%W(deployment-instructions deployment-logs #{deployment_group_id}))

  files_in_deployment_logs_folder = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/deployment-logs")
  expect(files_in_deployment_logs_folder.size).to eq(1)
  expect(files_in_deployment_logs_folder).to eq(%w(codedeploy-agent-deployments.log))

  directories_in_deployment_group_id_folder = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/#{deployment_group_id}")
  expect(directories_in_deployment_group_id_folder.size).to eq(1)
  expect(directories_in_deployment_group_id_folder).to eq([deployment_id])

  files_and_directories_in_deployment_id_folder = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/#{deployment_group_id}/#{deployment_id}")
  expect(files_and_directories_in_deployment_id_folder).to include(*%w(logs deployment-archive))

  files_and_directories_in_deployment_archive_folder = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/#{deployment_group_id}/#{deployment_id}/deployment-archive")
  expect(files_and_directories_in_deployment_archive_folder.size).to eq(2)
  expect(files_and_directories_in_deployment_archive_folder).to include(*%w(appspec.yml scripts))

  files_in_scripts_folder = directories_and_files_inside("#{InstanceAgent::Config.config[:root_dir]}/#{deployment_group_id}/#{deployment_id}/deployment-archive/scripts")
  sample_app_bundle_script_files = directories_and_files_inside(expected_scripts_directory)
  expect(files_in_scripts_folder.size).to eq(sample_app_bundle_script_files.size)
  expect(files_in_scripts_folder).to include(*sample_app_bundle_script_files)
end

Then(/^the scripts for events (.+) should have been executed and written to executed_proof_file in directory (\S+)$/) do |expected_executed_lifecycle_events_as_string, temp_test_directory|
  executed_proof_file_destination = "#{temp_test_directory}/executed_proof_file"

  file_lines = File.read(executed_proof_file_destination).split("\n")
  expected_executed_lifecycle_events = expected_executed_lifecycle_events_as_string.split(' ')

  expect(file_lines).to eq(expected_executed_lifecycle_events)
end
