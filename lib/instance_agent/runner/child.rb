# encoding: UTF-8
require 'process_manager/child'

module InstanceAgent
  module Runner
    class Child < ProcessManager::Daemon::Child
      AGENTS = {
        0 => InstanceAgent::CodeDeployPlugin::CommandPoller
      }

      attr_accessor :runner

      def prepare_run
        validate_index
        with_error_handling do
          @runner = AGENTS[index].runner
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
        raise ArgumentError, "Invalid index #{index.inspect}: only 0-2 possible" unless AGENTS.keys.include?(index)
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
