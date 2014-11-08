# encoding: UTF-8
module InstanceAgent
  module Agent
    class Base

      def self.runner
        self.new
      end

      def description
        self.class.to_s
      end

      def log(severity, message)
        raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
        InstanceAgent::Log.send(severity.to_sym, "#{description}: #{message}")
      end

      def run
        perform
      rescue Aws::Errors::MissingCredentialsError
        log(:error, "Missing credentials - please check if this instance was started with an IAM instance profile")
        sleep InstanceAgent::Config.config[:wait_after_error]
      rescue SocketError, Errno::ENETDOWN, Aws::Errors::ServiceError => e
        log(:error, "Cannot reach InstanceService: #{e.class} - #{e.message}")
        sleep InstanceAgent::Config.config[:wait_after_error]
      rescue Exception => e
        log(:error, "Error during perform: #{e.class} - #{e.message} - #{e.backtrace.join("\n")}")
        sleep InstanceAgent::Config.config[:wait_after_error]
      end

    end
  end
end
