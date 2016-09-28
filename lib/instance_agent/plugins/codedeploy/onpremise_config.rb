require 'instance_agent/file_credentials'

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
          return unless file_config

          raise "On Premises config cannot contain both 'iam_user_arn' and 'iam_session_arn' keys." if file_config[:iam_user_arn] and file_config[:iam_session_arn]
          if file_config[:iam_user_arn]
            [:region, :aws_access_key_id, :aws_secret_access_key].each do |field|
              raise "'#{field}' key is required when 'iam_user_arn' is provided." unless file_config[field]
            end
            ENV['AWS_REGION'] = file_config[:region]
            ENV['AWS_ACCESS_KEY'] = file_config[:aws_access_key_id]
            ENV['AWS_SECRET_KEY'] = file_config[:aws_secret_access_key]
            ENV['AWS_HOST_IDENTIFIER'] = file_config[:iam_user_arn]
          elsif file_config[:iam_session_arn]
            [:region, :aws_credentials_file].each do |field|
              raise "'#{field}' key is required when 'iam_session_arn' is provided." unless file_config[field]
            end
            ENV['AWS_REGION'] = file_config[:region]
            ENV['AWS_HOST_IDENTIFIER'] = file_config[:iam_session_arn]
            ENV['AWS_CREDENTIALS_FILE'] = file_config[:aws_credentials_file]
            Aws.config[:credentials] = InstanceAgent::FileCredentials.new(file_config[:aws_credentials_file])
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
