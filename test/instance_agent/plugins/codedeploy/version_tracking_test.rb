require 'test_helper'
require 'certificate_helper'
require 'stringio'
require 'aws-sdk-s3'

class CodeDeployVersionTrackingTest < InstanceAgentTestCase
  context 'CodeDeploy version tracking' do
    context 'on standard deployment root' do
      setup do
        ProcessManager::Config.config[:root_dir] = "/opt/codedeplopy/"
      end
      
      should 'be able to find the version file'do
        @version_tracker = Aws::Plugins::DeployAgentVersion::Handler.new()
        assert_not_equal nil, @version_tracker.get_version 
        assert_not_equal "UNKNOWN VERSION", @version_tracker.get_version 
      end
    end

    context 'on non standard deployment folder' do
      setup do
        ProcessManager::Config.config[:root_dir] = "/etc/"
      end
      
      should 'default to /opt/codedeploy-agent/.version' do
        @version_tracker = Aws::Plugins::DeployAgentVersion::Handler.new()
        assert_not_equal nil, @version_tracker.get_version 
        assert_not_equal "UNKNOWN VERSION", @version_tracker.get_version 
      end
    end
  end
end
