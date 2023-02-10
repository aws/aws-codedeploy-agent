require 'test_helper'
require 'certificate_helper'
require 'stringio'
require 'aws/codedeploy/local/deployer'

class UnpackBundleTest < InstanceAgentTestCase
  include InstanceAgent::Plugins::CodeDeployPlugin
  def generate_signed_message_for(map)
    message = @cert_helper.sign_message(map.to_json)
    spec = OpenStruct.new({ :payload => message })
    spec.format = "PKCS7/JSON"

    return spec
  end

  # method to create a local source bundle file as zip
  def setup_local_file_bundle
    @local_file_directory = File.join(@root_dir, @deployment_group_id.to_s, 'LocalFileDirectory')
    FileUtils.rm_rf(@local_file_directory)
    FileUtils.mkdir_p(@local_file_directory)

    input_filenames = %w(file1.txt file2.txt)
    input_filenames.each do |filename|
      File.open(File.join(@local_file_directory, filename), "w") do |f|
        f.write("content of #{filename}")
        f.close
      end
    end
    # create the bundle as a local zip file
    @local_file_location = File.join(@local_file_directory, "bundle.zip")
    Zip::File.open(@local_file_location, Zip::File::CREATE) do |zipfile|
      input_filenames.each do |filename|
        zipfile.add(filename, File.join(@local_file_directory, filename))
      end
    end
  end

  context 'The CodeDeploy Plugin Command Executor' do
    setup do
      @test_hook_mapping = {
          "BeforeBlockTraffic"=>["BeforeBlockTraffic"],
          "AfterBlockTraffic"=>["AfterBlockTraffic"],
          "ApplicationStop"=>["ApplicationStop"],
          "BeforeInstall"=>["BeforeInstall"],
          "AfterInstall"=>["AfterInstall"],
          "ApplicationStart"=>["ApplicationStart"],
          "BeforeAllowTraffic"=>["BeforeAllowTraffic"],
          "AfterAllowTraffic"=>["AfterAllowTraffic"],
          "ValidateService"=>["ValidateService"]
      }
      @deploy_control_client = mock
      @command_executor = InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor.new(
          {
              :deploy_control_client => @deploy_control_client,
              :hook_mapping => @test_hook_mapping
          })
      @aws_region = 'us-east-1'
      InstanceMetadata.stubs(:region).returns(@aws_region)
    end

    context "when executing a command" do
      setup do
        @cert_helper = CertificateHelper.new
        @deployment_id = SecureRandom.uuid
        @deployment_group_name = "TestDeploymentGroup"
        @application_name = "TestApplicationName"
        @deployment_group_id = "foobar"
        @command = Aws::CodeDeployCommand::Types::HostCommandInstance.new(
            :host_command_identifier => "command-1",
            :deployment_execution_id => "test-execution")
        @root_dir = '/tmp/codedeploy/'
        @deployment_root_dir = File.join(@root_dir, @deployment_group_id.to_s, @deployment_id.to_s)
        @archive_root_dir = File.join(@deployment_root_dir, 'deployment-archive')
        ProcessManager::Config.config[:root_dir] = @root_dir
      end

      context "test fallback mechanism in unpack_bundle in DownloadBundle" do
        setup do
          setup_local_file_bundle

          # Create a debris file in the deployment-archive directory to simulate Zip::DestinationFileExistsError.
          # This error will be thrown, if the ruby unzip overwrite option is not enabled and when the @archive_root_dir already has the same file.
          # With the ruby unzip overwrite fix, the unpack_bundle should succeed even with debris files.
          FileUtils.rm_rf(@archive_root_dir)
          FileUtils.mkdir_p(@archive_root_dir)
          FileUtils.cp(File.join(@local_file_directory, 'file1.txt'), @archive_root_dir)

          # We need to avoid removing @archive_root_dir in the actual logic, to avoid debris file to be deleted.
          FileUtils.stubs(:rm_rf).with(@archive_root_dir)
          # This exception will let the unpack_bundle method to use the rubyzip fallback mechanism
          InstanceAgent::LinuxUtil.expects(:extract_zip)
              .with(File.join(@deployment_root_dir, 'bundle.tar'), @archive_root_dir)
              .raises("Exception: System unzip throws exception with non-zero exit code")

          @command.command_name = 'DownloadBundle'
          @bundle_type = 'zip'
          @deployment_spec = generate_signed_message_for(
              {
                  "DeploymentId" => @deployment_id.to_s,
                  "DeploymentGroupId" => @deployment_group_id.to_s,
                  "ApplicationName" => @application_name,
                  "DeploymentGroupName" => @deployment_group_name,
                  "Revision" => {
                      "RevisionType" => "Local File",
                      "LocalRevision" => {
                          "Location" => @local_file_location,
                          "BundleType" => @bundle_type
                      }
                  }
              })
        end

        should 'execute DownloadBundle command with debris file in deployment-archive' do
          assert_equal 1, (Dir.entries(@archive_root_dir) - [".", ".."]).size
          @command_executor.execute_command(@command, @deployment_spec)
          assert_equal 2, (Dir.entries(@archive_root_dir) - [".", ".."]).size
        end
      end
    end
  end
end
