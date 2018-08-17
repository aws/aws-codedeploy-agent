# encoding: UTF-8
require 'process_manager/config'

module InstanceAgent
  class Config < ProcessManager::Config
    VALID_TIME_ZONES = ['local', 'utc']

    def self.init
      @config = Config.new
      ProcessManager::Config.instance_variable_set("@config", @config)
    end

    def validate
      errors = super
      validate_children(errors)
      validate_time_zone(errors)
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
        :ongoing_deployment_tracking => 'ongoing-deployment',
        :children => 1,
        :http_read_timeout => 80,
        :instance_service_region => nil,
        :instance_service_endpoint => nil,
        :instance_service_port => nil,
        :wait_between_runs => 30,
        :wait_after_error => 30,
        :time_zone => 'local',
        :codedeploy_test_profile => 'prod',
        :kill_agent_max_wait_time_seconds => 7200,
        :on_premises_config_file => '/etc/codedeploy-agent/conf/codedeploy.onpremises.yml',
        :proxy_uri => nil,
        :enable_deployments_log => true
      })
    end

    private

    def validate_children(errors = [])
      errors << 'children can only be set to 1' unless config[:children] == 1
      errors
    end

    def validate_time_zone(errors = [])
      errors << 'time_zone can only be set to [local|utc]' unless VALID_TIME_ZONES.include?(config[:time_zone])
      errors
    end

  end
end
