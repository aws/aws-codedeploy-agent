# encoding: UTF-8
require 'process_manager/config'
require 'set'

module InstanceAgent
  class Config < ProcessManager::Config
    
    FIPS_ENABLED_REGIONS = Set['us-east-1', 'us-east-2', 'us-west-1', 'us-west-2', 'us-gov-west-1', 'us-gov-east-1']  
    
    def self.init
      @config = Config.new
      ProcessManager::Config.instance_variable_set("@config", @config)
    end

    def validate
      errors = super
      validate_children(errors)
      validate_use_fips_mode(errors)
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
        :codedeploy_test_profile => 'prod',
        :kill_agent_max_wait_time_seconds => 7200,
        :on_premises_config_file => '/etc/codedeploy-agent/conf/codedeploy.onpremises.yml',
        :proxy_uri => nil,
        :enable_deployments_log => true,
        :use_fips_mode => false,
        :deploy_control_endpoint => nil
      })
    end

    def validate_children(errors = [])
      errors << 'children can only be set to 1' unless config[:children] == 1
    end
    
    def validate_use_fips_mode errors
      if config[:use_fips_mode] && ! (FIPS_ENABLED_REGIONS.include? region)
        errors << 'use_fips_mode can be set to true only in regions located in the USA' 
      end  
    end

    #Return the region we are currently in
    def region 
      ENV['AWS_REGION'] || InstanceMetadata.region
    end
    
  end
end
