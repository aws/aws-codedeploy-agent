require 'win32/daemon'
require 'core_ext'
require 'aws-sdk-core'
require 'process_manager'

# There's something strange about how Orca handles implicit requires.
# We have to explicitly require everything in advance or we'll get uninitialized constant failures.
require 'instance_agent/agent/base'
require 'instance_agent/config'
require 'instance_agent/log'
require 'instance_agent/platform'
require 'instance_agent/platform/windows_util'
require 'pathname'

include Win32

class InstanceAgentService < Daemon

  def initialize
    @app_root_folder = File.join(ENV['PROGRAMDATA'], "Amazon/CodeDeploy")
    InstanceAgent::Platform.util = InstanceAgent::WindowsUtil

    cert_dir = File.join(@app_root_folder, 'certs')
    Aws.config[:ssl_ca_bundle] = File.join(cert_dir, 'ca-bundle.crt')
    ENV['AWS_SSL_CA_DIRECTORY'] = File.join(cert_dir, 'ca-bundle.crt')
    ENV['SSL_CERT_FILE'] = File.join(cert_dir, 'ca-bundle.crt')
    @polling_mutex = Mutex.new
  end

  def description
    "CodeDeploy Instance Agent Service"
  end

  def service_main
    read_config
    log(:info, 'started')
    shutdown_flag = false
    while running? && !shutdown_flag
      with_error_handling do
        # Initialize the poller only once
        begin
          @polling_mutex.synchronize do
            @runner ||= InstanceAgent::Plugins::CodeDeployPlugin::CommandPoller.runner
            @runner.recover_from_crash?
            @runner.run
          end
        rescue SystemExit
          service_stop
          shutdown_flag = true
        end
        sleep InstanceAgent::Config.config[:wait_between_runs].to_i
      end
    end
    if shutdown_flag
      exit!
    end
  end
  
  def service_stop
    log(:info, 'stopping the agent')
    @polling_mutex.synchronize do
      @runner.graceful_shutdown
      log(:info, 'command execution threads shutdown, agent exiting now')
    end
  end
  
  def log(severity, message)
    raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
    InstanceAgent::Log.send(severity.to_sym, "#{description}: #{message}")
  end

  def expand_conf_path(key)
    tmp = InstanceAgent::Config.config[key.to_sym]
    InstanceAgent::Config.config(key.to_sym => File.join(ENV['PROGRAMDATA'], tmp)) unless Pathname.new(tmp).absolute?
  end 
  
  def read_config
    default_config = File.join(@app_root_folder, "conf.yml")
    InstanceAgent::Config.config({:config_file => default_config,
            :on_premises_config_file => File.join(@app_root_folder, "conf.onpremises.yml")})
    InstanceAgent::Config.load_config

    expand_conf_path(:root_dir)
    expand_conf_path(:log_dir)
    
    InstanceAgent::Log.init(File.join(InstanceAgent::Config.config[:log_dir], "codedeploy-agent-log.txt"))
  end
  
  def with_error_handling
    yield
  rescue SocketError => e
    log(:info, "#{description}: failed to run as the connection failed! #{e.class} - #{e.message} - #{e.backtrace.join("\n")}")
    sleep InstanceAgent::Config.config[:wait_after_connection_problem]
  rescue Exception => e
    if (e.message.to_s.match(/throttle/i) || e.message.to_s.match(/rateexceeded/i) rescue false)
      log(:error, "#{description}: ran into throttling - waiting for #{InstanceAgent::Config.config[:wait_after_throttle_error]}s until retrying")
      sleep InstanceAgent::Config.config[:wait_after_throttle_error]
    else
      log(:error, "#{description}: error during start or run: #{e.class} - #{e.message} - #{e.backtrace.join("\n")}")
      sleep 5
    end
  end  
end

# InstanceAgentService loads in a set of configurations that are assumed by the CodeDeploy plugin registery to be in place when the
# the classes are loaded. This happens to be true for the Linux agent as `GLI::App.pre` loads up the config before resolving additional
# gem dependencies. However, Aws.add_service requires that the service be defined when the gem is loaded so there is not an efficient way
# to resolve this at runtime; thus, the config must be resolved before the CodeDeployCommand class is loaded.
InstanceAgentService.new.read_config unless defined?(Ocra)
require 'instance_agent/plugins/codedeploy/register_plugin'

InstanceAgentService.mainloop unless defined?(Ocra)
