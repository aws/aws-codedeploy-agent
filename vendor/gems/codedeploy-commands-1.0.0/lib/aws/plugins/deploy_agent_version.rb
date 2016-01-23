module Aws
  module Plugins
    class DeployAgentVersion < Seahorse::Client::Plugin
      class Handler < Seahorse::Client::Handler
        def initialize(handler = nil)
          @handler = handler
          file_path = File.expand_path(File.join(InstanceAgent::Platform.util.codedeploy_version_file, '.version'))
          fallback_file_path = File.expand_path(File.join(InstanceAgent::Platform.util.fallback_version_file, '.version'))
          if File.exist?(file_path)
            @agent_version ||= File.read(file_path).split(': ').last.strip
            log(:info, "Version file found in #{file_path}.")
          elsif File.exist?(fallback_file_path)
            @agent_version ||= File.read(fallback_file_path).split(': ').last.strip
            log(:info, "Version file found in #{fallback_file_path}.")
          else 
            @agent_version ||= "UNKNOWN_VERSION"
            path_string = file_path.eql?(fallback_file_path)? "#{file_path}" : "#{file_path} or #{fallback_file_path}"
            log(:warn, "Version tracking file either does not exist or cannot be read in #{path_string}.")
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
