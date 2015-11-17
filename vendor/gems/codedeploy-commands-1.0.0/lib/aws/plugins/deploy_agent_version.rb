module Aws
  module Plugins
    class DeployAgentVersion < Seahorse::Client::Plugin
      class Handler < Seahorse::Client::Handler
        def initialize(handler = nil)
          @handler = handler
          file_path = get_version_tracking_file
          if File.exist?(file_path)
            @agent_version ||= File.read(file_path).split(': ').last.strip
          else 
            @agent_version ||= "UNKNOWN_VERSION"
            log(:warn, "Version tracking file either does not exist or cannot be read.")
          end
        end

        def call(context)
          context.http_request.headers['x-amz-codedeploy-agent-version'] = @agent_version
          @handler.call(context)
        end

        def get_version_tracking_file
          version_dir = ProcessManager::Config.config[:root_dir]
          if version_dir.eql? 'Amazon/CodeDeploy'
            file_path = File.join(version_dir, '.version')
          else 
            file_path = File.join(version_dir, '..', '.version')
          end
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
