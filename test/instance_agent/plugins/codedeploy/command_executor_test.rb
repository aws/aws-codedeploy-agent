require 'test_helper'
require 'certificate_helper'
require 'stringio'
require 'aws-sdk-s3'

require 'aws/codedeploy/local/deployer'

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
      @test_hook_mapping = { "BeforeBlockTraffic"=>["BeforeBlockTraffic"],
        "AfterBlockTraffic"=>["AfterBlockTraffic"],
        "ApplicationStop"=>["ApplicationStop"],
        "BeforeInstall"=>["BeforeInstall"],
        "AfterInstall"=>["AfterInstall"],
        "ApplicationStart"=>["ApplicationStart"],
        "BeforeAllowTraffic"=>["BeforeAllowTraffic"],
        "AfterAllowTraffic"=>["AfterAllowTraffic"],
        "ValidateService"=>["ValidateService"]}
      @deploy_control_client = mock
      @command_executor = InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor.new({
        :deploy_control_client => @deploy_control_client,
        :hook_mapping => @test_hook_mapping})
      @aws_region = 'us-east-1'
      @partition = 'aws'
      @domain = 'amazonaws.com'
      InstanceMetadata.stubs(:region).returns(@aws_region)
      InstanceMetadata.stubs(:partition).returns(@partition)
      InstanceMetadata.stubs(:domain).returns(@domain)
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
        @deployment_creator = "User"
        @deployment_type = "IN_PLACE"
        @s3Revision = {
          "Bucket" => "mybucket",
          "Key" => "mykey",
          "BundleType" => "tar"
        }
        @file_exists_behavior = "RETAIN"
        @agent_actions_overrides_map = {"FileExistsBehavior" => @file_exists_behavior}
        @agent_actions_overrides = {"AgentOverrides" => @agent_actions_overrides_map}
        @deployment_spec = generate_signed_message_for({
          "DeploymentId" => @deployment_id.to_s,
          "DeploymentGroupId" => @deployment_group_id,
          "ApplicationName" => @application_name,
          "DeploymentGroupName" => @deployment_group_name,
          "DeploymentCreator" => @deployment_creator,
          "DeploymentType" => @deployment_type,
          "AgentActionOverrides" => @agent_actions_overrides,
          "Revision" => {
            "RevisionType" => "S3",
            "S3Revision" => @s3Revision
          }
        })
        @command = Aws::CodeDeployCommand::Types::HostCommandInstance.new(
        :host_command_identifier => "command-1",
        :deployment_execution_id => "test-execution")
        @root_dir = '/tmp/codedeploy/'
        @deployment_root_dir = File.join(@root_dir, @deployment_group_id.to_s, @deployment_id.to_s)
        @deployment_instructions_dir =  File.join(@root_dir, 'deployment-instructions')
        @archive_root_dir = File.join(@deployment_root_dir, 'deployment-archive')
        ProcessManager::Config.config[:root_dir] = @root_dir

        FileUtils.stubs(:mkdir_p)
        File.stubs(:directory?).with(@deployment_root_dir).returns(true)
        @last_successful_install_file_location = File.join(@deployment_instructions_dir, "#{@deployment_group_id}_last_successful_install")
        @most_recent_install_file_location = File.join(@deployment_instructions_dir, "#{@deployment_group_id}_most_recent_install")
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
          File.stubs(:directory?).with(@deployment_instructions_dir).returns(true)
          File.stubs(:exist?).with(@last_successful_install_file_location).returns(true)
          File.stubs(:exist?).with(@archive_root_dir).returns(true)
          File.stubs(:open).with(@last_successful_install_file_location, 'w+')
          File.stubs(:open).with(@last_successful_install_file_location)

          @app_spec = mock("parsed application specification")
          File.
          stubs(:read).
          with("#@archive_root_dir/appspec.yml").
          returns("APP SPEC")
          ApplicationSpecification::ApplicationSpecification.stubs(:parse).with("APP SPEC").returns(@app_spec)
        end

        should "create an appropriate Installer" do
          Installer.
          expects(:new).
          with(:deployment_instructions_dir => @deployment_instructions_dir,
          :deployment_archive_dir => @archive_root_dir,
          :file_exists_behavior => @file_exists_behavior).
          returns(@installer)

          @command_executor.execute_command(@command, @deployment_spec)
        end

        should "perform the installation for the current IG, revision and app spec" do
          @installer.expects(:install).with(@deployment_group_id, @app_spec)

          @command_executor.execute_command(@command, @deployment_spec)
        end

        should "write the archive root dir to the install instructions file" do
          mock_file = mock
          File.expects(:open).with(@last_successful_install_file_location, 'w+').yields(mock_file)
          mock_file.expects(:write).with(@deployment_root_dir)

          @command_executor.execute_command(@command, @deployment_spec)
        end

        should 'raise ArgumentError if appspec contains unknown hook and deployment_spec includes all_possible_lifecycle_events' do
          all_possible_lifecycle_events = ['ExampleLifecycleEvent', 'SecondLifecycleEvent']
          deployment_spec = generate_signed_message_for({
            "DeploymentId" => @deployment_id.to_s,
            "DeploymentGroupId" => @deployment_group_id,
            "ApplicationName" => @application_name,
            "DeploymentGroupName" => @deployment_group_name,
            "DeploymentCreator" => @deployment_creator,
            "DeploymentType" => @deployment_type,
            "AgentActionOverrides" => @agent_actions_overrides,
            "AllPossibleLifecycleEvents" => all_possible_lifecycle_events,
            "Revision" => {
              "RevisionType" => "S3",
              "S3Revision" => @s3Revision
            }
          })

          app_spec = mock("parsed application specification")
          app_spec_hooks = {'UnknownHook' => nil}
          app_spec.expects(:hooks).returns(app_spec_hooks)
          File.stubs(:read).with("#@archive_root_dir/appspec.yml").returns("APP SPEC")
          ApplicationSpecification::ApplicationSpecification.stubs(:parse).with("APP SPEC").returns(app_spec)
          unknown_hooks = app_spec_hooks.merge(@test_hook_mapping)
          assert_raised_with_message("appspec.yml file contains unknown lifecycle events: #{unknown_hooks.keys}", ArgumentError) do
            @command_executor.execute_command(@command, deployment_spec)
          end
        end

        should 'raise ArgumentError if appspec custom hook specified that does not exist in appspec' do
          all_possible_lifecycle_events = AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS + ['ExampleLifecycleEvent', 'SecondLifecycleEvent', 'CustomHookNotInAppspec']
          deployment_spec = generate_signed_message_for({
            "DeploymentId" => @deployment_id.to_s,
            "DeploymentGroupId" => @deployment_group_id,
            "ApplicationName" => @application_name,
            "DeploymentGroupName" => @deployment_group_name,
            "DeploymentCreator" => @deployment_creator,
            "DeploymentType" => @deployment_type,
            "AgentActionOverrides" => @agent_actions_overrides,
            "AllPossibleLifecycleEvents" => all_possible_lifecycle_events,
            "Revision" => {
              "RevisionType" => "S3",
              "S3Revision" => @s3Revision
            }
          })

          app_spec = mock("parsed application specification")
          app_spec_hooks = InstanceAgent::Plugins::CodeDeployPlugin::CommandPoller::DEFAULT_HOOK_MAPPING.merge({'ExampleLifecycleEvent' => nil, 'SecondLifecycleEvent' => nil})
          app_spec.expects(:hooks).twice.returns(app_spec_hooks)
          File.stubs(:read).with("#@archive_root_dir/appspec.yml").returns("APP SPEC")
          ApplicationSpecification::ApplicationSpecification.stubs(:parse).with("APP SPEC").returns(app_spec)
          assert_raised_with_message("You specified a lifecycle event which is not a default one and doesn't exist in your appspec.yml file: CustomHookNotInAppspec", ArgumentError) do
            @command_executor.execute_command(@command, deployment_spec)
          end
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
          @s3.stubs(:config).returns("hello")
          Aws::S3::Client.stubs(:new).returns(@s3)
        end

        context "when GitHub revision specified" do
          setup do
            File.stubs(:directory?).with(@archive_root_dir).returns(true)
            FileUtils.stubs(:mv)
            FileUtils.stubs(:rmdir)
            @mock_file.stubs(:write)
            @deployment_spec = generate_signed_message_for({
              "DeploymentId" => @deployment_id.to_s,
              "DeploymentGroupId" => @deployment_group_id.to_s,
              "ApplicationName" => @application_name,
              "DeploymentGroupName" => @deployment_group_name,
              "Revision" => {
                "RevisionType" => "GitHub",
                "GitHubRevision" => {
                  'Account' => 'account',
                  'Repository' => 'repository',
                  'CommitId' => 'commitid',
                }
              }
            })

            ENV['AWS_SSL_CA_DIRECTORY'] = 'aws_ssl_ca_directory'
            @mock_uri = mock
            uri_options = {:ssl_verify_mode => OpenSSL::SSL::VERIFY_PEER, :redirect => true, :ssl_ca_cert => ENV['AWS_SSL_CA_DIRECTORY']}
            @mock_buffer = mock
            @mock_github_response = mock
            @mock_github_response.stubs(:read).returns(@mock_buffer)
            @mock_uri.stubs(:open).with(uri_options).yields(@mock_github_response)
          end

          should 'download file from github' do
            URI.expects(:parse).with("https://api.github.com/repos/account/repository/tarball/commitid").returns(@mock_uri)
            @command_executor.execute_command(@command, @deployment_spec)
          end

          context 'when Github bundle_type is specified' do
            setup do
              @bundle_type = 'zip'

              @deployment_spec = generate_signed_message_for({
                "DeploymentId" => @deployment_id.to_s,
                "DeploymentGroupId" => @deployment_group_id.to_s,
                "ApplicationName" => @application_name,
                "DeploymentGroupName" => @deployment_group_name,
                "Revision" => {
                  "RevisionType" => "GitHub",
                  "GitHubRevision" => {
                    'Account' => 'account',
                    'Repository' => 'repository',
                    'CommitId' => 'commitid',
                    'BundleType' => @bundle_type
                  }
                }
              })
            end

            should 'downloads from github with the corresponding format' do
              URI.expects(:parse).with("https://api.github.com/repos/account/repository/zipball/commitid").returns(@mock_uri)
              Zip::File.expects(:open).with(File.join(@deployment_root_dir, 'bundle.tar'))
              @command_executor.execute_command(@command, @deployment_spec)
            end
          end
        end

        context "when creating S3 options" do
          
          should "use right signature version" do 
            assert_equal 'v4', @command_executor.s3_options[:signature_version]
          end

          context "when override endpoint provided" do
            setup do
              InstanceAgent::Config.config[:s3_endpoint_override] = "https://example.override.endpoint.com"
            end
            should "use the override endpoint" do
              assert_equal "https://example.override.endpoint.com", @command_executor.s3_options[:endpoint].to_s
            end
          end
 
          context "when no override endpoint provided and not using fips" do
            setup do
              InstanceAgent::Config.config[:s3_endpoint_override] = nil
              InstanceAgent::Config.config[:use_fips_mode] = false
            end
            should "use correct region and custom endpoint" do
              assert_equal 'us-east-1', @command_executor.s3_options[:region]
              assert_false @command_executor.s3_options.include? :endpoint
            end
          end

          context "when no override endpoint provided and using fips" do
            setup do
              InstanceAgent::Config.config[:s3_endpoint_override] = nil
              InstanceAgent::Config.config[:use_fips_mode] = true
            end
            should "use correct region and custom endpoint" do
              assert_equal 'fips-us-east-1', @command_executor.s3_options[:region]
              assert_false @command_executor.s3_options.include? :endpoint
            end
          end      
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

        context "extract bundle from local file" do
          setup do
            InstanceAgent::LinuxUtil.stubs(:extract_tgz)
            @command.command_name = "DownloadBundle"
            @mock_file = mock
            @mock_file_location = '/mock/file/location.tgz'
            File.stubs(:symlink)
            Dir.stubs(:entries).returns []
            @mock_file.stubs(:close)

            @deployment_spec = generate_signed_message_for({
              "DeploymentId" => @deployment_id.to_s,
              "DeploymentGroupId" => @deployment_group_id.to_s,
              "ApplicationName" => @application_name,
              "DeploymentGroupName" => @deployment_group_name,
              "Revision" => {
                "RevisionType" => "Local File",
                "LocalRevision" => {
                  "Location" => @mock_file_location,
                  "BundleType" => 'tgz'
                }
              }
            })
          end

          should 'symlink the file to the bundle location' do
            File.expects(:symlink).with(@mock_file_location, File.join(@deployment_root_dir, 'bundle.tar'))
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "handle bundle from local directory" do
          setup do
            @command.command_name = "DownloadBundle"
            @mock_directory_location = '/mock/directory/location/'
            FileUtils.stubs(:cp_r)
            Dir.stubs(:entries).returns []
            @mock_file.stubs(:close)

            @deployment_spec = generate_signed_message_for({
              "DeploymentId" => @deployment_id.to_s,
              "DeploymentGroupId" => @deployment_group_id.to_s,
              "ApplicationName" => @application_name,
              "DeploymentGroupName" => @deployment_group_name,
              "Revision" => {
                "RevisionType" => "Local Directory",
                "LocalRevision" => {
                  "Location" => @mock_directory_location,
                  "BundleType" => 'directory'
                }
              }
            })
          end

          should 'copy recursively the directory to the bundle location' do
            FileUtils.expects(:cp_r).with(@mock_directory_location, @archive_root_dir)
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

        should "idempotently create the instructions directory" do
          FileUtils.expects(:mkdir_p).with(@deployment_instructions_dir)

          @command_executor.execute_command(@command, @deployment_spec)
        end

        should "write the archive root dir to the install instructions file" do
          mock_file = mock
          File.expects(:open).with(@most_recent_install_file_location, 'w+').yields(mock_file)
          mock_file.expects(:write).with(@deployment_root_dir)

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
            :deployment_creator => @deployment_creator,
            :deployment_type => @deployment_type,
            :deployment_root_dir => @deployment_root_dir, 
            :last_successful_deployment_dir => nil,
            :most_recent_deployment_dir => nil,
            :app_spec_path => 'appspec.yml'}
          @mock_hook_executor = mock
        end

        context "BeforeBlockTraffic" do
          setup do
            @command.command_name = "BeforeBlockTraffic"
            @hook_executor_constructor_hash[:lifecycle_event] = "BeforeBlockTraffic"
          end

          should "call execute a hook executor object with BeforeBlockTraffic as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "AfterBlockTraffic" do
          setup do
            @command.command_name = "AfterBlockTraffic"
            @hook_executor_constructor_hash[:lifecycle_event] = "AfterBlockTraffic"
          end

          should "call execute a hook executor object with AfterBlockTraffic as one of the params" do
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

        context "BeforeAllowTraffic" do
          setup do
            @command.command_name = "BeforeAllowTraffic"
            @hook_executor_constructor_hash[:lifecycle_event] = "BeforeAllowTraffic"
          end

          should "call execute a hook executor object with BeforeAllowTraffic as one of the params" do
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash).returns(@mock_hook_executor)
            @mock_hook_executor.expects(:execute)
            @command_executor.execute_command(@command, @deployment_spec)
          end
        end

        context "AfterAllowTraffic" do
          setup do
            @command.command_name = "AfterAllowTraffic"
            @hook_executor_constructor_hash[:lifecycle_event] = "AfterAllowTraffic"
          end

          should "call execute a hook executor object with AfterAllowTraffic as one of the params" do
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
          @command.command_name = "TestCommand"
          @test_hook_mapping = { "TestCommand" => ["lifecycle_event_1","lifecycle_event_2"]}
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
            :deployment_creator => @deployment_creator,
            :deployment_type => @deployment_type,
            :last_successful_deployment_dir => nil,
            :most_recent_deployment_dir => nil,
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
            HookExecutor.stubs(:new).with(@hook_executor_constructor_hash_1).raises("failed to create hook command")
          end

          should "calls lifecycle event 1 and fails but not lifecycle event 2" do
            assert_raised_with_message('failed to create hook command') do
              @command_executor.execute_command(@command, @deployment_spec)
            end
            HookExecutor.expects(:new).with(@hook_executor_constructor_hash_2).never
          end
        end
      end
    end
  end
end
