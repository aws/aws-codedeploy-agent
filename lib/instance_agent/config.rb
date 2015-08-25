# encoding: UTF-8
require 'process_manager/config'

module InstanceAgent
  class Config < ProcessManager::Config
    def self.init
      @config = Config.new
      ProcessManager::Config.instance_variable_set("@config", @config)
    end

    def validate
      errors = super
      validate_children(errors)
      errors
    end

    def initialize
      super
      @config.update({
        :program_name => 'codedeploy-agent',
        :wait_between_spawning_children => 1,
        :log_dir => nil,
        :pid_dir => nil,
        :shared_dir => nil,
        :user => nil,
        :children => 1,
        :http_read_timeout => 80,
        :instance_service_region => nil,
        :instance_service_endpoint => nil,
        :instance_service_port => nil,
        :wait_between_runs => 30,
        :wait_after_error => 30,
        :codedeploy_test_profile => 'prod',
        :on_premises_config_file => '/etc/codedeploy-agent/conf/codedeploy.onpremises.yml',
        :proxy_uri => nil
      })
    end

    def validate_children(errors = [])
      errors << 'children can only be set to 1' unless config[:children] == 1
      errors
    end

  end
end
