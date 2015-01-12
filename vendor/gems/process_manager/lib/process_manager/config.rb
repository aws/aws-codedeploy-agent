# encoding: UTF-8
module ProcessManager
  class Config

    def self.init
      @config = Config.new
    end

    def self.config(options = {})
      init unless @config
      @config.config(options)
    end

    def self.validate_config
      init unless @config
      @config.validate
    end

    def self.load_config
      if File.exists?(config[:config_file]) && File.readable?(config[:config_file])
        file_config = YAML.load(File.read(config[:config_file])).symbolize_keys
        config.update(file_config)
        config_loaded_callbacks.each{|c| c.call}
      else
        raise "The config file #{config[:config_file]} does not exist or is not readable"
      end
    end

    def self.on_config_load(&block)
      @@_config_callbacks ||= []
      @@_config_callbacks << block
      nil
    end

    def self.config_loaded_callbacks
      @@_config_callbacks ||= []
    end

    def initialize
      @config = {
        :program_name => 'process_manager',
        :max_runs_per_worker => 0, # unlimited
        :children => 4,
        :log_dir => '/tmp',
        :pid_dir => '/tmp',
        :verbose => false,
        :wait_after_throttle_error => 60, # wait time in seconds after a we got a throttling exception from SWF
        :wait_between_runs => 5, # wait time in seconds after a run so that we don't run into throttling exceptions
        :wait_after_connection_problem => 5, # wait time in seconds after a connection problem as we don't want to build a fork-bomb
        :wait_between_spawning_children => 10, # wait time in seconds after spawning a child so that we don't overhelm SWF with our requests
        :user => nil,
        :group => nil,

        # global config file to read
        :config_file => nil
      }
    end

    def config(options = {})
      @config.update(options) unless options.nil? || options.empty?
      @config
    end

    def validate
      errors = []

      errors << "Invalid max_runs_per_worker #{config[:max_runs_per_worker].inspect}" unless config[:max_runs_per_worker].to_s.match(/\d+/) && config[:max_runs_per_worker].to_i >= 0
      config[:max_runs_per_worker] = config[:max_runs_per_worker].to_i
      errors << "Invalid number of children #{config[:children].inspect}" unless config[:children].to_s.match(/\d+/) && config[:children].to_i > 0
      config[:children] = config[:children].to_i

      normalize_log_and_pid_dir
      validate_log_and_pid_dir(errors)
      validate_user(errors)
      errors
    end

    def validate_log_and_pid_dir(errors)
      FileUtils.mkdir_p(ProcessManager::Config.config[:log_dir]) unless File.exists?(ProcessManager::Config.config[:log_dir])
      FileUtils.mkdir_p(ProcessManager::Config.config[:pid_dir]) unless File.exists?(ProcessManager::Config.config[:pid_dir])
      errors << "Please make sure the path of the log directory exists and is writable: #{config[:log_dir].inspect}" unless file_writable?(config[:log_dir]) && File.directory?(config[:log_dir])
      errors << "Please make sure the path of the PID directory exists and is writable: #{config[:pid_dir].inspect}" unless file_writable?(config[:pid_dir]) && File.directory?(config[:pid_dir])
      errors
    end

    def validate_user(errors)
      if config[:user].present?
        errors << "The system user does not exist: #{config[:user].inspect}" unless (Etc.getpwnam(config[:user]).uid rescue false)
      end
      errors
    end

    def normalize_log_and_pid_dir
      if config[:pid_dir]
        config[:pid_dir] = File.expand_path(config[:pid_dir])
      end
      if config[:log_dir]
        config[:log_dir] = File.expand_path(config[:log_dir])
      end
    end

    def file_writable?(path)
      return false unless path.present?
      if File.exists?(path)
        File.writable?(path)
      else
        File.writable?(File.dirname(path))
      end
    end

  end
end
