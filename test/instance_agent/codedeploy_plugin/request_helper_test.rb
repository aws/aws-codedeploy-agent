require 'instance_metadata'
require 'aws/codedeploy_commands'

class CodeDeployRequestHelperTest < InstanceAgentTestCase

  include InstanceAgent::CodeDeployPlugin

  context "The CodeDeploy request helper" do

    context "when calling verify_clients_configuration" do
     setup do
        ENV['AWS_REGION'] = 'us-west-2'
        @deploy_client = mock('InstanceAgent::CodeDeployPlugin::CodeDeployControl')
        @deploy_client.stubs(:ssl_verify_peer).returns(true)
        @deploy_client.stubs(:verify_cert_fields).returns(true)

        @request_helper = RequestHelper.new(:deploy_control_client => @deploy_client)

      end

      should "successfully call verify_clients_configuration" do
        assert_equal(true, @request_helper.verify_clients_configuration.empty?)
      end

      should "fail if deploy client doesnt do ssl verify peer" do
        @deploy_client.stubs(:ssl_verify_peer).returns(false)
        assert_equal(false, @request_helper.verify_clients_configuration.empty?)
      end

      should "fail if deploy client cert verify failed" do
        @deploy_client.stubs(:verify_cert_fields).returns(false)
        assert_equal(false, @request_helper.verify_clients_configuration.empty?)
      end

    end
  end
end
