require 'test_helper'
require 'tempfile'

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
        :disable_imds_v1 => false,
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
        :on_premises_config_file => '/etc/codedeploy-agent/conf/codedeploy.onpremises.yml',
        :ongoing_deployment_tracking => 'ongoing-deployment',
        :proxy_uri => nil,
        :enable_deployments_log => true,
        :kill_agent_max_wait_time_seconds => 7200,
        :use_fips_mode => false,
        :deploy_control_endpoint => nil,
        :s3_endpoint_override => nil,
        :enable_auth_policy => false
      }, InstanceAgent::Config.config)
    end

    should 'be the same as the ProcessManager configuration for the current object' do
      config = InstanceAgent::Config.instance_variable_get(:@config)
      assert_equal config, ProcessManager::Config.instance_variable_get(:@config)
    end

    should 'execute all available validation methods' do
      InstanceMetadata.stubs(:region).returns('us-west-1')    #without stubbing this, the test will fail in the build fleet because MetadataService is not available there  
      validations = sequence('validation')
      err = []
      InstanceAgent::Config.any_instance.expects(:validate_children).with(err).in_sequence(validations)
      InstanceAgent::Config.any_instance.expects(:validate_use_fips_mode).with(err).in_sequence(validations)
      InstanceAgent::Config.validate_config
    end

    context 'validate configuration' do

      setup do
        InstanceAgent::Config.config[:instance_service_region] = 'eu-west-1'
        InstanceAgent::Config.config[:instance_service_endpoint] = 'api-endpoint.example.com'
        InstanceAgent::Config.config[:instance_service_port] = 123
          
        InstanceMetadata.stubs(:region).returns('us-west-1')    #without stubbing this, the test will fail in the build fleet because MetadataService is not available there  
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

    should 'return merged hash with common config options' do
      result = InstanceAgent::Config.common_client_config({
        :other => "test"
      })

      expected = {
        :other => "test",
        :instance_profile_credentials_retries => 3,
        :instance_profile_credentials_timeout => 1,  
      }

      assert_equal expected, result
    end
    
    context 'validate use_fips_mode' do
      
      error = 'use_fips_mode can be set to true only in regions located in the USA'
      
      should 'error in eu-west-1' do
        InstanceAgent::Config.config[:use_fips_mode] = true                
        ENV['AWS_REGION'] = 'eu-west-1'
        assert InstanceAgent::Config.validate_config.include? error    
      end
      
      should 'not error in eu-west-1 if not set' do
        InstanceAgent::Config.config[:use_fips_mode] = false
        ENV['AWS_REGION'] = 'eu-west-1'
        assert_false InstanceAgent::Config.validate_config.include? error    
      end
      
      should 'not error in us-east-1' do
        InstanceAgent::Config.config[:use_fips_mode] = true                
        ENV['AWS_REGION'] = 'us-east-1'
        assert_false InstanceAgent::Config.validate_config.include? error    
      end
      
      should 'not error in us-gov-west-1' do
        InstanceAgent::Config.config[:use_fips_mode] = true                
        ENV['AWS_REGION'] = 'us-gov-west-1'
        assert_false InstanceAgent::Config.validate_config.include? error    
      end
      
      cleanup do
        ENV['AWS_REGION'] = nil
      end

    end
  end

  context 'validate default config' do
    default_config_path = File.join(
      File.expand_path(File.dirname(__FILE__)),
      "../..",
      "conf",
      "codedeployagent.yml"
    )

    should 'load the default config file' do
      InstanceAgent::Config.config[:config_file] = default_config_path
      
      assert_equal(30, InstanceAgent::Config.config[:wait_between_runs])
      InstanceAgent::Config.load_config
      assert_equal(1, InstanceAgent::Config.config[:wait_between_runs])
    end

    should 'include a newline at the end of the file' do
      lines = []
      File.open(default_config_path).each_line do |line|
        lines << line
      end

      # note that newline is not the last line but the last character at the end of the last line
      assert_equal("\n", lines[-1][-1])
    end
  end

  context 'config loading logic' do
    should 'use the last config entry in the file' do
      config_file = Tempfile.new("config.yml")
      begin
        config_file.write <<~FILE
          ---
          :string_param: testing_one
          :string_param: testing_two
          :boolean_param: false
          :boolean_param: true
          :number_param: 1
          :number_param: 2

        FILE

        config_file.close

        InstanceAgent::Config.config[:config_file] = config_file.path
        InstanceAgent::Config.load_config

        assert_equal("testing_two", InstanceAgent::Config.config[:string_param])
        assert_equal(true, InstanceAgent::Config.config[:boolean_param])
        assert_equal(2, InstanceAgent::Config.config[:number_param])
      ensure
        config_file.delete
      end
    end

    should 'handle gaps in the file config' do
      config_file = Tempfile.new("config.yml")
      begin
        config_file.write <<~FILE
          ---

          :param_one: one

          :param_two: two

          :param_three: three

        FILE

        config_file.close

        InstanceAgent::Config.config[:config_file] = config_file.path
        InstanceAgent::Config.load_config

        assert_equal("one", InstanceAgent::Config.config[:param_one])
        assert_equal("two", InstanceAgent::Config.config[:param_two])
        assert_equal("three", InstanceAgent::Config.config[:param_three])
      ensure
        config_file.delete
      end
    end

    should 'raise readable error on config load failure' do
      config_file = Tempfile.new("config.yml")
      begin
        config_file.write <<~FILE
          this is not valid
        FILE

        config_file.close

        InstanceAgent::Config.config[:config_file] = config_file.path
        exception = assert_raise(RuntimeError) { InstanceAgent::Config.load_config }

        message = exception.to_s

        assert_match(/^An error occurred loading the CodeDeploy agent config file at #{config_file.path}. Error message:.*$/, message)
      ensure
        config_file.delete
      end
    end

    should 'raise readable error on config file not found' do
      fake_path = "/path/does/not/exist/not_here.yml"
      InstanceAgent::Config.config[:config_file] = "/path/does/not/exist/not_here.yml"
      exception = assert_raise(RuntimeError) { InstanceAgent::Config.load_config }

      message = exception.to_s

      assert_match(/^The config file #{fake_path} does not exist or is not readable$/, message)
    end
  end
end
