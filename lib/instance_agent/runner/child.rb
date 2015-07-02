# encoding: UTF-8
require 'process_manager/child'

module InstanceAgent
  module Runner
    class Child < ProcessManager::Daemon::Child

      attr_accessor :runner

      def load_plugins(plugins)
        ProcessManager::Log.debug("Registering Plugins: #{plugins.inspect}.")
        plugins.each do |plugin|
          plugin_dir = File.expand_path("../plugins/#{plugin}/register_plugin", File.dirname(__FILE__))
          ProcessManager::Log.debug("Loading plugin #{plugin} from #{plugin_dir}")
          begin
            require plugin_dir
          rescue LoadError => e
            ProcessManager::Log.error("Plugin #{plugin} could not be loaded: #{e.message}.")
            raise
          end
        end
        registered_plugins = InstanceAgent::Agent::Base.plugins
        ProcessManager::Log.debug("Registered Plugins: #{registered_plugins.inspect}.")
        Hash[registered_plugins.map.with_index { |value, index| [index, value] }]
      end

      def prepare_run
        @plugins ||= load_plugins(ProcessManager::Config.config[:plugins] || ["codedeploy"])
        validate_index
        with_error_handling do
          @runner = @plugins[index].runner
          ProcessManager.set_program_name(description)
        end
      end

      def run
        with_error_handling do
          runner.run
        end
      end

      def description
        if runner
          "#{runner.description} of master #{master_pid.inspect}"
        else
          'booting child'
        end
      end

      def validate_index
        raise ArgumentError, "Invalid index #{index.inspect}" unless @plugins.keys.include?(index)
      end

      def with_error_handling
        yield
      rescue SocketError => e
        ProcessManager::Log.info "#{description}: failed to run as the connection failed! #{e.class} - #{e.message} - #{e.backtrace.join("\n")}"
        sleep ProcessManager::Config.config[:wait_after_connection_problem]
        exit 1
      rescue Exception => e
        if (e.message.to_s.match(/throttle/i) || e.message.to_s.match(/rateexceeded/i) rescue false)
          ProcessManager::Log.error "#{description}: ran into throttling - waiting for #{ProcessManager::Config.config[:wait_after_throttle_error]}s until retrying"
          sleep ProcessManager::Config.config[:wait_after_throttle_error]
        else
          ProcessManager::Log.error "#{description}: error during start or run: #{e.class} - #{e.message} - #{e.backtrace.join("\n")}"
        end
        exit 1
      end

    end
  end
end
