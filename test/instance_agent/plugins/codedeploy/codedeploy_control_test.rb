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
        InstanceMetadata.stubs(:imds_supported?).returns(true)
        InstanceMetadata.stubs(:region).returns('us-west-2')
        InstanceMetadata.stubs(:domain).returns('amazonaws.com')
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

      context "with CodeDeploy endpoint set in an environment variable" do
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

      context "with enable_auth_policy set" do
        setup do
          InstanceAgent::Config.config[:enable_auth_policy] = true
        end

        should "use secure endpoint" do
          codedeploy_control_client = CodeDeployControl.new :region => "us-west-2"
          assert_equal "codedeploy-commands-secure.us-west-2.amazonaws.com", codedeploy_control_client.get_client.config.endpoint.host
        end
      end

      context "with both of use_fips_mode and enable_auth_policy set" do
        setup do
          InstanceAgent::Config.config[:use_fips_mode] = true
          InstanceAgent::Config.config[:enable_auth_policy] = true
        end

        should "use secure Fips endpoint" do
          codedeploy_control_client = CodeDeployControl.new :region => "us-west-2"
          assert_equal "codedeploy-commands-secure-fips.us-west-2.amazonaws.com", codedeploy_control_client.get_client.config.endpoint.host
        end
      end

      context "without IMDS" do
        setup do
          InstanceMetadata.stubs(:imds_supported?).returns(false)
          InstanceMetadata.stubs(:region).returns(nil)
          InstanceMetadata.stubs(:domain).returns(nil)
        end

        should "use the config defined settings" do
          codedeploy_control_client = CodeDeployControl.new :region => "us-east-1"
          assert_equal "codedeploy-commands.us-east-1.amazonaws.com", codedeploy_control_client.get_client.config.endpoint.host
        end

        should "resolve non .com domains" do
          codedeploy_control_client = CodeDeployControl.new :region => "cn-north-1"
          assert_equal "codedeploy-commands.cn-north-1.amazonaws.com.cn", codedeploy_control_client.get_client.config.endpoint.host
        end
      end

      context "common config" do
        should "add common config to the client settings" do
          client = CodeDeployControl.new :region => "us-east-1"
          assert_equal 3, client.get_client.config.instance_profile_credentials_retries
        end
      end
    end
  end
end
