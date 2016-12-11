require 'instance_metadata'

class CodeDeployControlTest < InstanceAgentTestCase

  include InstanceAgent::Plugins::CodeDeployPlugin

  context "The CodeDeploy interface client" do

    context "when initializing" do
      setup do
        ENV['AWS_ACCESS_KEY_ID'] = "Test Access Key"
        ENV['AWS_SECRET_ACCESS_KEY'] = "Test Secret Access Key"
        ENV['AWS_REGION'] = nil
        ENV['AWSDEPLOY_CONTROL_ENDPOINT'] = "https://tempuri"
        ENV['DEPLOYMENT_CREATOR'] = "User"
        ENV['DEPLOYMENT_TYPE'] = "IN_PLACE"
      end

      context "with region, endpoint and credentials" do
        should "successfully initialize" do
          codedeploy_control_client = CodeDeployControl.new(:region => "us-west-2")
          codedeploy_control_client.get_client
        end
      end

      context "without a region" do
        should "raise an exception" do
          assert_raise {
            codedeploy_control_client = CodeDeployControl.new()
            codedeploy_control_client.get_client.put_host_command_complete(
              :command_status => 'Succeeded',
              :diagnostics => nil,
              :host_command_identifier => "TestCommand")
          }
        end
      end

      context "without an endpoint" do
        setup do
          ENV['AWS_DEPLOY_CONTROL_ENDPOINT'] = nil
        end

        should "raise an exception" do
          assert_raise {
            codedeploy_control_client = CodeDeployControl.new(:region => "us-west-2")
            codedeploy_control_client.get_client.put_host_command_complete(
              :command_status => 'Succeeded',
              :diagnostics => nil,
              :host_command_identifier => "TestCommand")

          }
        end
      end
    end
  end
end
