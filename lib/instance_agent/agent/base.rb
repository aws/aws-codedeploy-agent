# encoding: UTF-8
require 'instance_agent/agent/plugin'

module InstanceAgent
  module Agent
    class Base
      include InstanceAgent::Agent::Plugin

      def self.runner
        instance = self.new
        instance.validate if instance.respond_to?('validate')
        instance
      end

      def description
        self.class.to_s
      end

      def log(severity, message)
        raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
        InstanceAgent::Log.send(severity.to_sym, "#{description}: #{message}")
      end

      def run
        start_time = Time.now

        begin
          perform
          @error_count = 0
        rescue Aws::Errors::MissingCredentialsError
          log(:error, "Missing credentials - please check if this instance was started with an IAM instance profile")
          @error_count = @error_count.to_i + 1
        rescue SocketError, Errno::ENETDOWN, Aws::Errors::ServiceError => e
          log(:error, "Cannot reach InstanceService: #{e.class} - #{e.message}")
          @error_count = @error_count.to_i + 1
        rescue Exception => e
          log(:error, "Error during perform: #{e.class} - #{e.message} - #{e.backtrace.join("\n")}")
          @error_count = @error_count.to_i + 1
        end

        if @error_count > 0
          # Max out at 90 seconds between calls and take 5 minutes before reaching the cap and allowing 10 calls to get there

          if @error_count > 10
            @error_count = 10
          end

          elapsed_time = (Time.now - start_time).ceil
          backoff_time = (((1.2675 ** @error_count) * (90.0 / (1.2675 ** 10)))).floor
          sleep_time = backoff_time - elapsed_time

          if(sleep_time > 0)
            log(:debug, "Sleeping #{sleep_time} seconds.")
            sleep sleep_time
          end
        end
      end
    end
  end
end
