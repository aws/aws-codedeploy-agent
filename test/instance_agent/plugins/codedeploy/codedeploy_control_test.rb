require 'instance_metadata'

class CodeDeployControlTest < InstanceAgentTestCase

  include InstanceAgent::Plugins::CodeDeployPlugin

  context "The CodeDeploy interface client" do

    context "when initializing" do
      setup do
        ENV['AWS_ACCESS_KEY_ID'] = "Test Access Key"
        ENV['AWS_SECRET_ACCESS_KEY'] = "Test Secret Access Key"
        ENV['AWS_REGION'] = nil
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
          InstanceAgent::Config.config[:deploy_control_endpoint] = nil
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

      context "with ADCS endpoint set in an environment variable" do
        setup do
          InstanceAgent::Config.config[:deploy_control_endpoint] = "https://tempuri"
        end

        should "use endpoint from environment variable" do
            codedeploy_control_client = CodeDeployControl.new :region => "us-west-2"
            assert_equal "tempuri", codedeploy_control_client.get_client.config.endpoint.host
        end
        
        cleanup do
         InstanceAgent::Config.config[:deploy_control_endpoint] = nil
        end
      end            
      
      context "with use_fips_mode not set" do
        should "use non-Fips endpoint" do
            codedeploy_control_client = CodeDeployControl.new :region => "us-west-2"
            assert_equal "codedeploy-commands.us-west-2.amazonaws.com", codedeploy_control_client.get_client.config.endpoint.host
        end
      end
            
      context "with use_fips_mode set" do
        setup do
          InstanceAgent::Config.config[:use_fips_mode] = true
        end

        should "use Fips endpoint" do
            codedeploy_control_client = CodeDeployControl.new :region => "us-west-2"
            assert_equal "codedeploy-commands-fips.us-west-2.amazonaws.com", codedeploy_control_client.get_client.config.endpoint.host
        end
      end
        
    end
  end
end
