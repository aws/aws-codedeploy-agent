require 'process_manager/log'
require 'singleton'

InstanceAgent::Log = ProcessManager::Log

class InstanceAgent::DeploymentLog
  include Singleton

  def initialize
    deployment_logs_dir = File.join(InstanceAgent::Config.config[:root_dir], 'deployment-logs')
    FileUtils.mkdir_p(deployment_logs_dir) unless File.exist? deployment_logs_dir
    @deployment_log ||= Logger.new(File.join(deployment_logs_dir, "#{InstanceAgent::Config.config[:program_name]}-deployments.log"), 8, 64 * 1024 * 1024)
    @deployment_log.formatter = proc do |severity, datetime, progname, msg|
      "[#{datetime.strftime('%Y-%m-%d %H:%M:%S.%L')}] #{msg}\n"
    end
  end

  def log(message)
    @deployment_log.info(message)
  end  
end
