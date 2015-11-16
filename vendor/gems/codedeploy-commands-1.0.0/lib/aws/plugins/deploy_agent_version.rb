module Aws
  module Plugins
    class DeployAgentVersion < Seahorse::Client::Plugin
      class Handler < Seahorse::Client::Handler
        def initialize(handler = nil)
          @handler = handler
          file_path = File.join(ProcessManager::Config.config[:version_dir], '.version')
          if File.exist?(file_path)
            @agent_version ||= File.read(file_path).split(': ').last.strip
          else 
            @agent_version ||= "UNKNOWN_VERSION"
            log(:warn, "Version tracking file doesn't exist in directory #{file_path}")
          end
        end

        def call(context)
          context.http_request.headers['x-amz-codedeploy-agent-version'] = @agent_version
          @handler.call(context)
        end
        
        private
        def log(severity, message)
          raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
          InstanceAgent::Log.send(severity.to_sym, "#{message}")
        end
      end

      handler(Handler)
    end
  end
end
