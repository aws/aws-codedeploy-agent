require 'test_helper'
require 'json'

class CommandPollerTest < InstanceAgentTestCase

  def gather_diagnostics_from_error(error)
    {'error_code' => InstanceAgent::Plugins::CodeDeployPlugin::ScriptError::UNKNOWN_ERROR_CODE, 'script_name' => "", 'message' => error.message, 'log' => ""}.to_json
  end

  def gather_diagnostics(script_output)
    script_output ||= ""
    {'error_code' => InstanceAgent::Plugins::CodeDeployPlugin::ScriptError::SUCCEEDED_CODE, 'script_name' => "", 'message' => "Succeeded", 'log' => script_output}.to_json
  end

  context 'The command poller' do

    setup do
      @host_identifier = "i-123"
      @aws_region = 'us-east-1'
      @deploy_control_endpoint = "my-deploy-control.amazon.com"
      @deploy_control_client = mock()
      @deploy_control_api = mock()
      @executor = stub(:execute_command => "test this is not returned",
                       :deployment_system => "CodeDeploy")

      ENV['AWS_REGION'] = nil
      InstanceMetadata.stubs(:region).returns(@aws_region)
      InstanceMetadata.stubs(:host_identifier).returns(@host_identifier)

      InstanceAgent::Plugins::CodeDeployPlugin::OnPremisesConfig.stubs(:configure)
      InstanceAgent::Plugins::CodeDeployPlugin::CodeDeployControl.stubs(:new).
        returns(@deploy_control_api)
      @deploy_control_api.stubs(:get_client).
        returns(@deploy_control_client)

      InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor.stubs(:new).
        returns(@executor)

      @poller = InstanceAgent::Plugins::CodeDeployPlugin::CommandPoller.new
    end

    context 'on initializing' do

      should 'construct a client using the configured region' do
        InstanceAgent::Plugins::CodeDeployPlugin::CodeDeployControl.expects(:new).
          with(has_entries(:region => @aws_region)).
          returns(@deploy_control_api)

        InstanceAgent::Plugins::CodeDeployPlugin::CommandPoller.new
      end

      should 'construct an CodeDeploy command executor' do
        test_hook_mapping = { "BeforeBlockTraffic"=>["BeforeBlockTraffic"],
          "AfterBlockTraffic"=>["AfterBlockTraffic"],
          "ApplicationStop"=>["ApplicationStop"],
          "BeforeInstall"=>["BeforeInstall"],
          "AfterInstall"=>["AfterInstall"],
          "ApplicationStart"=>["ApplicationStart"],
          "BeforeAllowTraffic"=>["BeforeAllowTraffic"],
          "AfterAllowTraffic"=>["AfterAllowTraffic"],
          "ValidateService"=>["ValidateService"]}
        InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor.expects(:new).
          with(:hook_mapping => test_hook_mapping).
          returns(@executor)

        InstanceAgent::Plugins::CodeDeployPlugin::CommandPoller.new
      end

    end

    context 'on perform' do

      setup do
        @command = stub(
          :host_command_identifier => "my-host-command-identifier",
          :command_name => "DownloadBundle",
          :host_identifier => @host_identifier,
          :deployment_execution_id => "command-deployment-execution-id")
        @poll_host_command_output = stub(:host_command => @command)
        @poll_host_command_acknowledgement_output = stub(:command_status => "InProgress")
        @deployment_specification = stub(:generic_envelope => '{"some":"json"}')
        @get_deploy_specification_output = stub(
          :deployment_system => "CodeDeploy",
          :deployment_specification => @deployment_specification)

        @poll_host_command_state = states('poll_host_command_state').
          starts_as('setup')
        @deploy_control_client.stubs(:poll_host_command).
          returns(@poll_host_command_output).
          when(@poll_host_command_state.is('setup'))

        @put_host_command_acknowledgement_state = states('put_host_command_acknowledgement_state').
        starts_as('setup')
        @deploy_control_client.stubs(:put_host_command_acknowledgement).
          returns(@poll_host_command_acknowledgement_output).
          when(@put_host_command_acknowledgement_state.is('setup'))

        @get_deployment_specification_state = states('get_deployment_specification_state').
          starts_as('setup')
        @deploy_control_client.stubs(:get_deployment_specification).
          returns(@get_deploy_specification_output).
          when(@get_deployment_specification_state.is('setup'))

        @execute_command_state = states('execute_command_state').
          starts_as('setup')
        @executor.stubs(:execute_command).
          when(@execute_command_state.is('setup'))

        @put_host_command_complete_state = states('put_host_command_complete_state').
          starts_as('setup')
        @deploy_control_client.stubs(:put_host_command_complete).
          when(@put_host_command_complete_state.is('setup'))
      end

      should 'call PollHostCommand with the current host name' do
        @deploy_control_client.expects(:poll_host_command).
          with(:host_identifier => @host_identifier).
          returns(@poll_host_command_output)

        @poller.perform
      end

      should 'return when no command is given by PollHostCommand' do
        @deploy_control_client.expects(:poll_host_command).
          with(:host_identifier => @host_identifier).
          returns(stub(:host_command => nil))

        @put_host_command_acknowledgement_state.become('never')
        @deploy_control_client.expects(:put_host_command_acknowledgement).never.
          when(@put_host_command_acknowledgement_state.is('never'))
        @get_deployment_specification_state.become('never')
        @deploy_control_client.expects(:get_deployment_specification).never.
          when(@get_deployment_specification_state.is('never'))
        @put_host_command_complete_state.become('never')
        @deploy_control_client.expects(:put_host_command_complete).never.
          when(@put_host_command_complete_state.is('never'))

        @poller.perform
      end

      should 'raise expection when a different host identifier given by PollHostCommand' do
        command = stub(
          :host_command_identifier => "my-host-command-identifier",
          :command_name => "DownloadBundle",
          :host_identifier => "different-host-identifier",
          :deployment_execution_id => "command-deployment-execution-id")

        poll_host_command_output = stub(:host_command => command)

        @deploy_control_client.expects(:poll_host_command).
          with(:host_identifier => @host_identifier).
          returns(poll_host_command_output)

        @put_host_command_acknowledgement_state.become('never')
        @deploy_control_client.expects(:put_host_command_acknowledgement).never.
          when(@put_host_command_acknowledgement_state.is('never'))
        @get_deployment_specification_state.become('never')
        @deploy_control_client.expects(:get_deployment_specification).never.
          when(@get_deployment_specification_state.is('never'))
        @put_host_command_complete_state.become('never')
        @deploy_control_client.expects(:put_host_command_complete).never.
          when(@put_host_command_complete_state.is('never'))

        assert_raise do
          @poller.perform
        end
      end

      should 'Accept a host name that is a substring of the actual host name' do
        command = stub(
          :host_command_identifier => "my-host-command-identifier",
          :command_name => "DownloadBundle",
          :host_identifier => @host_identifier[0],
          :deployment_execution_id => "command-deployment-execution-id")

        poll_host_command_output = stub(:host_command => command)

        @deploy_control_client.expects(:poll_host_command).
          with(:host_identifier => @host_identifier).
          returns(poll_host_command_output)

        @poller.perform
      end

      should 'raise exception when no command name is given by PollHostCommand' do
        command = stub(
          :host_command_identifier => "my-host-command-identifier",
          :command_name => nil,
          :host_identifier => @host_identifier,
          :deployment_execution_id => "command-deployment-execution-id")

        poll_host_command_output = stub(:host_command => command)

        @deploy_control_client.expects(:poll_host_command).
          with(:host_identifier => @host_identifier).
          returns(poll_host_command_output)

        @put_host_command_acknowledgement_state.become('never')
        @deploy_control_client.expects(:put_host_command_acknowledgement).never.
          when(@put_host_command_acknowledgement_state.is('never'))
        @get_deployment_specification_state.become('never')
        @deploy_control_client.expects(:get_deployment_specification).never.
          when(@get_deployment_specification_state.is('never'))
        @put_host_command_complete_state.become('never')
        @deploy_control_client.expects(:put_host_command_complete).never.
          when(@put_host_command_complete_state.is('never'))

        assert_raise do
          @poller.perform
        end
      end

      should 'raise exception when empty command name is given by PollHostCommand' do
        command = stub(
          :host_command_identifier => "my-host-command-identifier",
          :command_name => "",
          :host_identifier => @host_identifier,
          :deployment_execution_id => "command-deployment-execution-id")

        poll_host_command_output = stub(:host_command => command)

        @deploy_control_client.expects(:poll_host_command).
          with(:host_identifier => @host_identifier).
          returns(poll_host_command_output)

        @put_host_command_acknowledgement_state.become('never')
        @deploy_control_client.expects(:put_host_command_acknowledgement).never.
          when(@put_host_command_acknowledgement_state.is('never'))
        @get_deployment_specification_state.become('never')
        @deploy_control_client.expects(:get_deployment_specification).never.
          when(@get_deployment_specification_state.is('never'))
        @put_host_command_complete_state.become('never')
        @deploy_control_client.expects(:put_host_command_complete).never.
          when(@put_host_command_complete_state.is('never'))

        assert_raise do
          @poller.perform
        end
      end

      should 'allow exceptions from PollHostCommand to propagate to caller' do
        @deploy_control_client.stubs(:poll_host_command).
          raises("some error")

        assert_raise "some error" do
          @poller.perform
        end
      end

      should 'call PollHostCommandAcknowledgement with host_command_identifier returned by PollHostCommand' do
        @deploy_control_client.expects(:put_host_command_acknowledgement).
          with(:diagnostics => nil,
               :host_command_identifier => @command.host_command_identifier).
          returns(@poll_host_command_acknowledgement_output)

        @poller.perform
      end

      should 'return when Succeeded command status is given by PollHostCommandAcknowledgement' do
        @deploy_control_client.expects(:put_host_command_acknowledgement).
          with(:diagnostics => nil,
               :host_command_identifier => @command.host_command_identifier).
          returns(stub(:command_status => "Succeeded"))

        @get_deployment_specification_state.become('never')
        @deploy_control_client.expects(:get_deployment_specification).never.
          when(@get_deployment_specification_state.is('never'))
        @put_host_command_complete_state.become('never')
        @deploy_control_client.expects(:put_host_command_complete).never.
          when(@put_host_command_complete_state.is('never'))

        @poller.perform
      end

      should 'return when Failed command status is given by PollHostCommandAcknowledgement' do
        @deploy_control_client.expects(:put_host_command_acknowledgement).
          with(:diagnostics => nil,
               :host_command_identifier => @command.host_command_identifier).
          returns(stub(:command_status => "Failed"))

        @get_deployment_specification_state.become('never')
        @deploy_control_client.expects(:get_deployment_specification).never.
          when(@get_deployment_specification_state.is('never'))
        @put_host_command_complete_state.become('never')
        @deploy_control_client.expects(:put_host_command_complete).never.
          when(@put_host_command_complete_state.is('never'))

        @poller.perform
      end

      should 'call GetDeploymentSpecification with the host ID and execution ID of the command' do
        @deploy_control_client.expects(:get_deployment_specification).
          with(:deployment_execution_id => @command.deployment_execution_id,
               :host_identifier => @host_identifier).
          returns(@get_deploy_specification_output)

        @poller.perform
      end

      should 'allow exceptions from GetDeploymentSpecification to propagate to caller' do
        @deploy_control_client.expects(:get_deployment_specification).
          raises("some error")

        assert_raise "some error" do
          @poller.perform
        end
      end

      context 'when an empty deployment system is given by GetDeploymentSpecification' do

        setup do
          @get_deploy_specification_output.stubs(:deployment_system).
            returns("")
        end

        should 'not dispatch the command to the command executor' do
          @execute_command_state.become('never')
          @executor.expects(:execute_command).never.
            when(@execute_command_state.is('never'))

          assert_raise do
            @poller.perform
          end
        end

        should 'call put_host_command_complete with a status of Failed' do

          @deploy_control_client.expects(:put_host_command_complete).
            with(:command_status => "Failed",
                 :diagnostics => {:format => "JSON", :payload => gather_diagnostics_from_error(RuntimeError.new("Deployment System mismatch: CodeDeploy != "))},
                 :host_command_identifier => @command.host_command_identifier)

          assert_raise do
            @poller.perform
          end
        end

      end

      context 'when the wrong deployment system is given by GetDeploymentSpecification' do

        setup do
          @get_deploy_specification_output.stubs(:deployment_system).
            returns("WackyDeployer")
        end

        should 'not dispatch the command to the command executor' do
          @execute_command_state.become('never')
          @executor.expects(:execute_command).never.
            when(@execute_command_state.is('never'))

          assert_raise do
            @poller.perform
          end
        end

        should 'call put_host_command_complete with a status of Failed' do
          @deploy_control_client.expects(:put_host_command_complete).
            with(:command_status => "Failed",
                 :diagnostics => {:format => "JSON", :payload => gather_diagnostics_from_error(RuntimeError.new("Deployment System mismatch: CodeDeploy != WackyDeployer"))},
                 :host_command_identifier => @command.host_command_identifier)

          assert_raise do
            @poller.perform
          end
        end

      end

      context 'when no deployment specification is given by GetDeploymentSpecification' do

        setup do
          @get_deploy_specification_output.stubs(:deployment_specification).
            returns(nil)
        end

        should 'not dispatch the command to the command executor' do
          @execute_command_state.become('never')
          @executor.expects(:execute_command).never.
            when(@execute_command_state.is('never'))

          assert_raise do
            @poller.perform
          end
        end

        should 'call PutHostCommandComplete with a status of Failed' do
          @deploy_control_client.expects(:put_host_command_complete).
            with(:command_status => "Failed",
                 :diagnostics => {:format => "JSON", :payload => gather_diagnostics_from_error(RuntimeError.new("Deployment Specification missing"))},
                 :host_command_identifier => @command.host_command_identifier)

          assert_raise do
            @poller.perform
          end
        end

      end

      should 'dispatch the command to the command executor' do
        @executor.expects(:execute_command).
          with(@command, @deployment_specification.generic_envelope)

        @poller.perform
      end

      should 'allow exceptions from execute_command to propagate to caller' do
        @executor.expects(:execute_command).
          raises("some error")

        @deploy_control_client.expects(:put_host_command_complete).
          with(:command_status => "Failed",
               :diagnostics => {:format => "JSON", :payload => gather_diagnostics_from_error(RuntimeError.new("some error"))},
               :host_command_identifier => @command.host_command_identifier)

        assert_raise "some error" do
          @poller.perform
        end
      end

      should 'allow script errors from execute_command to propagate diagnostic information to caller' do
        begin
          script_log = InstanceAgent::Plugins::CodeDeployPlugin::ScriptLog.new
          script_log.append_to_log("log entries")
          raise InstanceAgent::Plugins::CodeDeployPlugin::ScriptError.new(InstanceAgent::Plugins::CodeDeployPlugin::ScriptError::SCRIPT_FAILED_CODE, "file_location", script_log), 'message'
        rescue InstanceAgent::Plugins::CodeDeployPlugin::ScriptError => e
          script_error = e
        end

        @executor.expects(:execute_command).
          raises(script_error)

        @deploy_control_client.expects(:put_host_command_complete).
          with(:command_status => "Failed",
               :diagnostics => {:format => "JSON", :payload => script_error.to_json},
               :host_command_identifier => @command.host_command_identifier)

        assert_raise script_error do
          @poller.perform
        end
      end

      should 'complete the command when the command executor successfully processes the command' do
        @executor.expects(:execute_command).
          with(@command, @deployment_specification.generic_envelope)

        @deploy_control_client.expects(:put_host_command_complete).
          with(:command_status => "Succeeded",
                :diagnostics => {:format => "JSON", :payload => gather_diagnostics("")},
                :host_command_identifier => @command.host_command_identifier)

        @poller.perform
      end

    end

  end

end
