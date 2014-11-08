require 'aws/codedeploy_commands'
require 'httpclient'
require 'instance_metadata'

module InstanceAgent
  module CodeDeployPlugin
    class CodeDeployControl

      def initialize(options = {})
        @options = options.update({
          :http_read_timeout => InstanceAgent::Config.config[:http_read_timeout]
        })

        if InstanceAgent::Config.config[:log_aws_wire]
          @options = options.update({
            # wire logs might be huge; customers should be careful about turning them on
            # allow 1GB of old wire logs in 64MB chunks
            :logger => Logger.new(
              File.join(InstanceAgent::Config.config[:log_dir], "#{InstanceAgent::Config.config[:program_name]}.aws_wire.log"),
              16,
              64 * 1024 * 1024),
            :http_wire_trace => true})
        end
      end

      def get_client
        Aws::CodeDeployCommand::Client.new(@options)
      end

      def ssl_verify_peer
        get_client.config.ssl_verify_peer
      end

      def verify_cert_fields
          deploy_control_endpoint = get_client.config.endpoint
        begin
          cert_verifier = InstanceAgent::CodeDeployPlugin::CodeDeployControlCertVerifier.new(deploy_control_endpoint)
          cert_verifier.verify_subject
        rescue e
          InstanceAgent::Log.error("#{self.class.to_s}: Error during certificate verification on codedeploy endpoint #{deploy_control_endpoint}")
          InstanceAgent::Log.debug("#{self.class.to_s}: #{e.inspect}")
          false
        end
      end
    end

    class CodeDeployControlCertVerifier

      def initialize(endpoint)
        client = HTTPClient.new
        response = client.get(endpoint)
        @cert = response.peer_cert
        @region = ENV['AWS_REGION'] || InstanceMetadata.region
      end

      def verify_subject
        InstanceAgent::Log.debug("#{self.class.to_s}: Actual certificate subject is '#{@cert.subject.to_s}'")

        case @region
          when 'us-east-1'
            @cert.subject.to_s == "/C=US/ST=Washington/L=Seattle/O=Amazon.com, Inc./CN=codedeploy-commands.us-east-1.amazonaws.com"
          when 'us-west-2'
            @cert.subject.to_s == "/C=US/ST=Washington/L=Seattle/O=Amazon.com, Inc./CN=codedeploy-commands.us-west-2.amazonaws.com"
          else
            InstanceAgent::Log.debug("#{self.class.to_s}: Unsupported region '#{@region}'")
            false
        end
      end

    end
  end
end
