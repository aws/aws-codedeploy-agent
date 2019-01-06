# 1.9 adds realpath to resolve symlinks; 1.8 doesn't
# have this method, so we add it so we get resolved symlinks
# and compatibility
unless File.respond_to? :realpath
  class File #:nodoc:
    def self.realpath path
      return realpath(File.readlink(path)) if symlink?(path)
      path
    end
  end
end

# Set the environment variables for ruby libs and vendor provided gems
# This is required so that the agent can run without requiring an init script
# if installed as a gem

agent_dir = "/opt/codedeploy-agent"
$:.unshift *Dir.glob("#{agent_dir}/vendor/gems/**/lib")
$:.unshift "#{agent_dir}/lib"
# Required for integration tests to run correctly
$:.unshift File.join(File.dirname(File.expand_path('..', __FILE__)), 'lib')

require 'instance_agent'
require 'gli'

include GLI::App

program_desc 'AWS CodeDeploy Agent'

conf_default_dir = "/etc/codedeploy-agent/conf/codedeployagent.yml"
conf_repo_dir = "#{agent_dir}/conf/codedeployagent.yml"
desc 'Path to agent config file'
if File.file?(conf_default_dir)
  default_value conf_default_dir
else
  default_value conf_repo_dir
end
arg_name "conf_dir"
flag [:config_file,:config_file]

desc 'start the AWS CodeDeploy agent'
command :start do |c|
  c.action do |global_options,options,args|
    InstanceAgent::Runner::Master.start
  end
end

desc 'stop the AWS CodeDeploy agent'
command :stop do |c|
  c.action do |global_options,options,args|
    InstanceAgent::Runner::Master.stop
    if pid = InstanceAgent::Runner::Master.status
      raise 'AWS CodeDeploy agent is still running'
    end
  end
end

desc 'restart the AWS CodeDeploy agent'
command :restart do |c|
  c.action do |global_options,options,args|
    InstanceAgent::Runner::Master.restart
  end
end

desc 'Report running status of the AWS CodeDeploy agent'
command :status do |c|
  c.action do |global_options,options,args|
    if pid = InstanceAgent::Runner::Master.status
      puts "The AWS CodeDeploy agent is running as PID #{pid}"
    else
      raise 'No AWS CodeDeploy agent running'
    end
  end
end

pre do |global,command,options,args|
  InstanceAgent::Config.config.keys.each do |config_key|
    InstanceAgent::Config.config(config_key => global[config_key]) if global[config_key].present?
  end

  InstanceAgent::Platform.util = InstanceAgent::LinuxUtil

  InstanceAgent::Config.load_config
  true
end

on_error do |exception|
  true
end

exit run(ARGV)
