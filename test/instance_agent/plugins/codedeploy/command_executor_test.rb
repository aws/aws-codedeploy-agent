require 'test_helper'
require 'certificate_helper'
require 'stringio'
require 'aws-sdk-core/s3'

class CodeDeployPluginCommandExecutorTest < InstanceAgentTestCase

  include InstanceAgent::Plugins::CodeDeployPlugin
      def generate_signed_message_for(map)
        message = @cert_helper.sign_message(map.to_json)
        spec = OpenStruct.new({ :payload => message })
        spec.format = "PKCS7/JSON"

        return spec
      end

  context 'The CodeDeploy Plugin Command Executor' do
    setup do
      @test_hook_mapping = { "BeforeELBRemove"=>["BeforeELBRemove"],
        "AfterELBRemove"=>["AfterELBRemove"],
        "ApplicationStop"=>["ApplicationStop"],
        "BeforeInstall"=>["BeforeInstall"],
        "AfterInstall"=>["AfterInstall"],
        "ApplicationStart"=>["ApplicationStart"],
        "BeforeELBAdd"=>["BeforeELBAdd"],
        "AfterELBAdd"=>["AfterELBAdd"],
        "ValidateService"=>["ValidateService"]}
      @deploy_control_client = mock
      @command_executor = InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor.new({
        :deploy_control_client => @deploy_control_client,
        :hook_mapping => @test_hook_mapping})
      @aws_region = 'us-east-1'
      InstanceMetadata.stubs(:region).returns(@aws_region)
    end

    context "deployment_system method" do
      should "always return CodeDeploy" do
        assert_equal "CodeDeploy", @command_executor.deployment_system
      end
    end

    context "when executing a command" do

      setup do
        @cert_helper = CertificateHelper.new
        @deployment_id = SecureRandom.uuid
        @deployment_group_name = "TestDeploymentGroup"
        @application_name = "TestApplicationName"
        @deployment_group_id = "foo"
        @s3Revision = {
          "Bucket" => "mybucket",
          "Key" => "mykey",
          "BundleType" => "tar"
        }
        @deployment_spec = generate_signed_message_for({
          "DeploymentId" => @deployment_id.to_s,
          "DeploymentGroupId" => @deployment_group_id,
          "ApplicationName" => @application_name,
          "DeploymentGroupName" => @deployment_group_name,
          "Revision" => {
            "RevisionType" => "S3",
            "S3Revision" => @s3Revision
          }
        })
        @command = OpenStruct.new(
        :host_command_identifier => "command-1",
        :deployment_execution_id => "test-execution")
        @root_dir = '/tmp/codedeploy/'
        @deployment_root_dir = File.join(@root_dir, @deployment_group_id.to_s, @deployment_id.to_s)
        @deployment_instructions_dir =  File.join(@root_dir, 'deployment-instructions')
        @archive_root_dir = File.join(@deployment_root_dir, 'deployment-archive')
        ProcessManager::Config.config[:root_dir] = @root_dir

        FileUtils.stubs(:mkdir_p)
        File.stubs(:directory?).with(@deployment_root_dir).returns(true)
        @previous_install_file_location = File.join(@deployment_instructions_dir, "#{@deployment_group_id}_last_successful_install")
      end

      context "when executing an unknown command" do
        setup do
          @command.command_name = "unknown-command"
        end

        should "not create the deployment root directory" do
          # Need to unstub the :mkdir_p method otherwise the never expectation doesn't work
          FileUtils.unstub(:mkdir_p)
          FileUtils.expects(:mkdir_p).never

          assert_raised_with_message('Unsupported command type: unknown-command.', InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor::InvalidCommandNameFailure) do
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        should "throw an exception" do
          assert_raised_with_message('Unsupported command type: unknown-command.', InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor::InvalidCommandNameFailure) do
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end
      end

      context "when executing a valid command" do
        setup do
          @command.command_name = "Install"
          @command_executor.stubs(:install)
        end

        should "create the deployment root directory" do
          FileUtils.expects(:mkdir_p).with(@deployment_root_dir)

          @command_executor.execute_command(@command, @deployment_spec)
        end

        context "when failed to create root directory" do
          setup do
            File.stubs(:directory?).with(@deployment_root_dir).returns(false)
          end

          should "raise an exception" do
            assert_raised_with_message("Error creating deployment root directory #{@deployment_root_dir}") do
              @command_executor.execute_command(@command, @deployment_spec)
            end
          end
        end
      end

      context "when executing the Install command" do

        setup do
          @command.command_name = "Install"
          InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::ApplicationSpecification.stubs(:parse).returns(@app_spec)
          @installer = stub("installer", :install => nil)
          Installer.stubs(:new).returns(@installer)
          File.stubs(:exist?).with(@previous_install_file_location).returns(true)
          File.stubs(:exist?).with(@archive_root_dir).returns(true)
          File.stubs(:open).with(@previous_install_file_location, 'w+')
          File.stubs(:open).with(@previous_install_file_location)

          @app_spec = mock("parsed application specification")
          File.
          stubs(:read).
          with("#@archive_root_dir/appspec.yml").
          returns("APP SPEC")
          ApplicationSpecification::ApplicationSpecification.stubs(:parse).with("APP SPEC").returns(@app_spec)
        end

        should "idempotently create the instructions directory" do
          FileUtils.expects(:mkdir_p).with(@deployment_instructions_dir)

          @command_executor.execute_command(@command, @deployment_spec)
        end

        should "create an appropriate Installer" do
          Installer.
          expects(:new).
          with(:deployment_instructions_dir => @deployment_instructions_dir,
          :deployment_archive_dir => @archive_root_dir).
          returns(@installer)

          @command_executor.execute_command(@command, @deployment_spec)
        end

        should "perform the installation for the current IG, revision and app spec" do
          @installer.expects(:install).with(@deployment_group_id, @app_spec)

          @command_executor.execute_command(@command, @deployment_spec)
        end

        should "write the archive root dir to the install instructions file" do
          mock_file = mock
          File.expects(:open).with(@previous_install_file_location, 'w+').yields(mock_file)
          mock_file.expects(:write).with(@deployment_root_dir)

          @command_executor.execute_command(@command, @deployment_spec)
        end

      end

      context "when executing the DownloadBundle command" do
        setup do
          InstanceAgent::LinuxUtil.stubs(:extract_tar)
          InstanceAgent::LinuxUtil.stubs(:extract_tgz)
          @command.command_name = "DownloadBundle"
          @http = mock
          @mock_file = mock
          Net::HTTP.stubs(:start).yields(@http)
          File.stubs(:open).returns @mock_file
          Dir.stubs(:entries).returns []
          @mock_file.stubs(:close)
          @http.stubs(:request_get)
          @s3 = mock
          Aws::S3::Client.stubs(:new).returns(@s3)
        end

        context "downloading bundle from S3" do
          setup do
            File.expects(:open).with(File.join(@deployment_root_dir, 'bundle.tar'), 'wb').yields(@mock_file)
            @object = mock
            @s3.stubs(:get_object).returns(@object)
            @io = mock
            @object.stubs(:etag).returns("myetag")
            @deployment_spec = generate_signed_message_for({
              "DeploymentId" => @deployment_id.to_s,
              "DeploymentGroupId" => @deployment_group_id.to_s,
              "ApplicationName" => @application_name,
              "DeploymentGroupName" => @deployment_group_name,
              "Revision" => {
                "RevisionType" => "S3",
                "S3Revision" => {
                  "Bucket" => "mybucket",
                  "Key" => "mykey",
                  "BundleType" => "tar"
                }
              }
            })
          end

          context "when setting up the S3 client" do
            setup do
              ENV['AWS_REGION'] = nil
              InstanceMetadata.stubs(:region).returns('us-east-1')
            end

            should "read from the InstanceMetadata to get the region" do
              InstanceMetadata.expects(:region)
              @command_executor.execute_command(@command, @deployment_spec)
            end
          end

          should "verify etag" do
            @deployment_spec = generate_signed_message_for({
              "DeploymentId" => @deployment_id.to_s,
              "DeploymentGroupId" => @deployment_group_id.to_s,
              "ApplicationName" => @application_name,
              "DeploymentGroupName" => @deployment_group_name,
              "Revision" => {
                "RevisionType" => "S3",
                "S3Revision" => {
                  "Bucket" => "mybucket",
                  "Key" => "mykey",
                  "BundleType" => "tar",
                  "ETag" => "myetag"
                }
              }
            })
            @command_executor.execute_command(@command, @deployment_spec)
          end

          should "verify etag that contains quotations still matches" do
            @object.stubs(:etag).returns("\"myetag\"")
            @deployment_spec = generate_signed_message_for({
              "DeploymentId" => @deployment_id.to_s,
              "DeploymentGroupId" => @deployment_group_id.to_s,
              "ApplicationName" => @application_name,
              "DeploymentGroupName" => @deployment_group_name,
              "Revision" => {
                "RevisionType" => "S3",
                "S3Revision" => {
                  "Bucket" => "mybucket",
                  "Key" => "mykey",
                  "BundleType" => "tar",
                  "ETag" => "myetag"
                }
              }
            })
            @command_executor.execute_command(@command, @deployment_spec)
          end

          should "verify version" do
            @object.stubs(:body).with(:bucket => "mybucket", :key => "mykey", :version_id => "myversion").returns(@io)
            @deployment_spec = generate_signed_message_for({
              "DeploymentId" => @deployment_id.to_s,
              "DeploymentGroupId" => @deployment_group_id.to_s,
              "ApplicationName" => @application_name,
              "DeploymentGroupName" => @deployment_group_name,
              "Revision" => {
                "RevisionType" => "S3",
                "S3Revision" => {
                  "Bucket" => "mybucket",
                  "Key" => "mykey",
                  "BundleType" => "tar",
                  "Version" => "myversion"
                }
              }
            })
            @command_executor.execute_command(@command, @deployment_spec)
          end

          should "call zip for zip BundleTypes" do
            @deployment_spec = generate_signed_message_for({
              "DeploymentId" => @deployment_id.to_s,
              "DeploymentGroupId" => @deployment_group_id.to_s,
              "ApplicationName" => @application_name,
              "DeploymentGroupName" => @deployment_group_name,
              "Revision" => {
                "RevisionType" => "S3",
                "S3Revision" => {
                  "Bucket" => "mybucket",
                  "Key" => "mykey",
                  "BundleType" => "zip"
                }
              }
            })
            Zip::File.expects(:open).with(File.join(@deployment_root_dir, 'bundle.tar'))
            @command_executor.execute_command(@command, @deployment_spec)
          end

          should "call extract_tgz for Gzipped tar BundleTypes" do
            @deployment_spec = generate_signed_message_for({
              "DeploymentId" => @deployment_id.to_s,
              "DeploymentGroupId" => @deployment_group_id.to_s,
              "ApplicationName" => @application_name,
              "DeploymentGroupName" => @deployment_group_name,
              "Revision" => {
                "RevisionType" => "S3",
                "S3Revision" => {
                  "Bucket" => "mybucket",
                  "Key" => "mykey",
                  "BundleType" => "tgz"
                }
              }
            })
            InstanceAgent::LinuxUtil.expects(:extract_tgz).with(File.join(@deployment_root_dir, 'bundle.tar'), @archive_root_dir)
            @command_executor.execute_command(@command, @deployment_spec)
          end

          should "call extract_tar for tar BundleTypes" do
            @deployment_spec = generate_signed_message_for({
              "DeploymentId" => @deployment_id.to_s,
              "DeploymentGroupId" => @deployment_group_id.to_s,
              "ApplicationName" => @application_name,
              "DeploymentGroupName" => @deployment_group_name,
              "Revision" => {
                "RevisionType" => "S3",
                "S3Revision" => {
                  "Bucket" => "mybucket",
                  "Key" => "mykey",
                  "BundleType" => "tar"
                }
              }
            })
            InstanceAgent::LinuxUtil.expects(:extract_tar).with(File.join(@deployment_root_dir, 'bundle.tar'), @archive_root_dir)
            @command_executor.execute_command(@command, @deployment_spec)
          end

        end

        should "unpack the bundle to the right directory" do
          InstanceAgent::LinuxUtil.expects(:extract_tar).with(File.join(@deployment_root_dir, 'bundle.tar'), @archive_root_dir)
          @command_executor.execute_command(@command, @deployment_spec)
        end

        should "remove the directory before unpacking" do
          call_sequence = sequence("call sequence")
          FileUtils.expects(:rm_rf).with(@archive_root_dir).in_sequence(call_sequence)
          InstanceAgent::LinuxUtil.expects(:extract_tar).in_sequence(call_sequence)
          @command_executor.execute_command(@command, @deployment_spec)
        end
      end

      context "I have an empty app spec (for script mapping)" do
        setup do
          File.stubs(:read).with(File.join(@archive_root_dir, 'appspec.yml')).returns(nil)
          InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::ApplicationSpecification.stubs(:parse).returns(nil)
          @hook_executor_constructor_hash = {
            :application_name => @application_name,
            :deployment_id => @deployment_id,
            :deployment_group_name => @deployment_group_name,
            :deployment_group_id => @deployment_group_id,
            :deployment_root_dir => @deployment_root_dir,
            :last_successful_deployment_dir => nil,
            :app_spec_path => 'appspec.yml'}
          @mock_hook_executor = mock
        end

        context "BeforeELBRemove" do
          setup do
            @command.command_name = "BeforeELBRemove"
            @hook_executor_constructor_hash[:lifecycle_event] = "BeforeELBRemove"
          end

          should "call execute a hook executor object with BeforeELBRemove as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "AfterELBRemove" do
          setup do
            @command.command_name = "AfterELBRemove"
            @hook_executor_constructor_hash[:lifecycle_event] = "AfterELBRemove"
          end

          should "call execute a hook executor object with AfterELBRemove as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "ApplicationStop" do
          setup do
            @command.command_name = "ApplicationStop"
            @hook_executor_constructor_hash[:lifecycle_event] = "ApplicationStop"
          end

          should "call execute a hook executor object with ApplicationStop as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "BeforeInstall" do
          setup do
            @command.command_name = "BeforeInstall"
            @hook_executor_constructor_hash[:lifecycle_event] = "BeforeInstall"
          end

          should "call execute a hook executor object with BeforeInstall as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "AfterInstall" do
          setup do
            @command.command_name = "AfterInstall"
            @hook_executor_constructor_hash[:lifecycle_event] = "AfterInstall"
          end

          should "call execute a hook executor object with AfterInstall as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "ApplicationStart" do
          setup do
            @command.command_name = "ApplicationStart"
            @hook_executor_constructor_hash[:lifecycle_event] = "ApplicationStart"
          end

          should "call execute a hook executor object with ApplicationStart as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "BeforeELBAdd" do
          setup do
            @command.command_name = "BeforeELBAdd"
            @hook_executor_constructor_hash[:lifecycle_event] = "BeforeELBAdd"
          end

          should "call execute a hook executor object with BeforeELBAdd as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "AfterELBAdd" do
          setup do
            @command.command_name = "AfterELBAdd"
            @hook_executor_constructor_hash[:lifecycle_event] = "AfterELBAdd"
          end

          should "call execute a hook executor object with AfterELBAdd as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "ValidateService" do
          setup do
            @command.command_name = "ValidateService"
            @hook_executor_constructor_hash[:lifecycle_event] = "ValidateService"
          end

          should "call execute a hook executor object with ValidateService as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end
      end

      #non 1:1 mapping tests
      context "one command hooks to multiple lifecycle events" do
        setup do
          @command.command_name = "test_command"
          @test_hook_mapping = { "test_command" => ["lifecycle_event_1","lifecycle_event_2"]}
          @deploy_control_client = mock
          @command_executor = InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor.new({
            :deploy_control_client => @deploy_control_client,
            :hook_mapping => @test_hook_mapping})
          hook_executor_constructor_hash = {
            :deployment_root_dir => @deployment_root_dir,
            :application_name => @application_name,
            :deployment_id => @deployment_id,
            :deployment_group_name => @deployment_group_name,
            :deployment_group_id => @deployment_group_id,
            :last_successful_deployment_dir => nil,
            :app_spec_path => 'appspec.yml'}
          @hook_executor_constructor_hash_1 = hook_executor_constructor_hash.merge({:lifecycle_event => "lifecycle_event_1"})
          @hook_executor_constructor_hash_2 = hook_executor_constructor_hash.merge({:lifecycle_event => "lifecycle_event_2"})
          @mock_hook_executor = mock
        end

        should "call both lifecycle events" do
          HookExecutor.expects(:new).with(@hook_executor_constructor_hash_1).returns(@mock_hook_executor)
          HookExecutor.expects(:new).with(@hook_executor_constructor_hash_2).returns(@mock_hook_executor)
          @mock_hook_executor.expects(:execute).twice

          @command_executor.execute_command(@command, @deployment_spec)
        end

        context "when the first script is forced to fail" do
          setup do
            HookExecutor.stubs(:new).with(@hook_executor_constructor_hash_1).raises("failed to create hook caommand")

          end

          should "calls lifecycle event 1 and fails but not 2" do
            assert_raised_with_message('failed to create hook caommand') do
              @command_executor.execute_command(@command, @deployment_spec)
            end
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash_2).never
          end
        end
      end
    end
  end
end
