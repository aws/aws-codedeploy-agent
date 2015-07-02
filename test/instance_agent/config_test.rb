require 'test_helper'

class InstanceAgentConfigTest < InstanceAgentTestCase
  context 'The instance agent configuration' do

    should 'have a default configuration' do
      InstanceAgent::Config.init
      assert_equal({
        :wait_between_spawning_children => 1,
        :log_dir => nil,
        :pid_dir => nil,
        :shared_dir => nil,
        :user => nil,
        :group=>nil,
        :program_name => "codedeploy-agent",
        :wait_after_throttle_error => 60,
        :wait_between_runs => 30,
        :verbose => false,
        :config_file => nil,
        :wait_after_connection_problem => 5,
        :children => 1,
        :max_runs_per_worker => 0,
        :http_read_timeout => 80,
        :instance_service_region => nil,
        :instance_service_endpoint => nil,
        :instance_service_port => nil,
        :wait_between_runs => 30,
        :wait_after_error => 30,
        :codedeploy_test_profile => 'prod',
        :on_premises_config_file => '/etc/codedeploy-agent/conf/codedeploy.onpremises.yml'
      }, InstanceAgent::Config.config)
    end

    should 'be the same as the ProcessManager configuration for the current object' do
      config = InstanceAgent::Config.instance_variable_get(:@config)
      assert_equal config, ProcessManager::Config.instance_variable_get(:@config)
    end

    should 'execute all available validation methods' do
      validations = sequence('validation')
      err = []
      InstanceAgent::Config.any_instance.expects(:validate_children).with(err).in_sequence(validations)
      InstanceAgent::Config.validate_config
    end

    context 'validate configuration' do

      setup do
        InstanceAgent::Config.config[:instance_service_region] = 'eu-west-1'
        InstanceAgent::Config.config[:instance_service_endpoint] = 'api-endpoint.example.com'
        InstanceAgent::Config.config[:instance_service_port] = 123
      end

      should 'validate the children setting' do
        InstanceAgent::Config.config[:children] = nil
        puts InstanceAgent::Config.config.inspect
        assert_equal 'children can only be set to 1', InstanceAgent::Config.validate_config.pop
        InstanceAgent::Config.config[:children] = 2
        assert_equal 'children can only be set to 1', InstanceAgent::Config.validate_config.pop
        InstanceAgent::Config.config[:children] = 1
        assert InstanceAgent::Config.validate_config.empty?, InstanceAgent::Config.validate_config.inspect
      end
    end
  end
end
