module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      class OnPremisesConfig
        def self.configure
          file_path = InstanceAgent::Config.config[:on_premises_config_file]
          file_config = nil
          if File.exists?(file_path) && File.readable?(file_path)
            begin
              file_config = YAML.load(File.read(file_path)).symbolize_keys
            rescue
              log(:error, "Invalid on premises config file")
              raise "Invalid on premises config file"
            end
          else
            log(:info, "On Premises config file does not exist or not readable")
          end
          if file_config
            if file_config[:region]
              ENV['AWS_REGION'] = file_config[:region]
            end
            if file_config[:aws_access_key_id]
              ENV['AWS_ACCESS_KEY'] = file_config[:aws_access_key_id]
            end
            if file_config[:aws_secret_access_key]
              ENV['AWS_SECRET_KEY'] = file_config[:aws_secret_access_key]
            end
            if file_config[:iam_user_arn]
              ENV['AWS_HOST_IDENTIFIER'] = file_config[:iam_user_arn]
            end
          end
        end

        def self.log(severity, message)
          raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
          InstanceAgent::Log.send(severity.to_sym, "#{message}")
        end

      end
    end
  end
end