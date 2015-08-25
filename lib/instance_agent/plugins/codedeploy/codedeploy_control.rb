require 'aws/codedeploy_commands'
require 'net/http'
require 'openssl'
require 'instance_metadata'

module InstanceAgent
  module Plugins
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

          if InstanceAgent::Config.config[:proxy_uri]
            @options = options.update({
              :http_proxy => URI(InstanceAgent::Config.config[:proxy_uri]) })
          end
        end

        def validate_ssl_config
          errors = []
          errors << "Invalid aws sdk security configuration" unless ssl_verify_peer
          errors << "Invalid server certificate" unless verify_cert_fields
          errors.each{|error| InstanceAgent::Log.error("Error validating the SSL configuration: " + error)}
          errors.empty?
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
            cert_verifier = InstanceAgent::Plugins::CodeDeployPlugin::CodeDeployControlCertVerifier.new(deploy_control_endpoint)
            cert_verifier.verify_cert
          rescue Exception => e
            InstanceAgent::Log.error("#{self.class.to_s}: Error during certificate verification on codedeploy endpoint #{deploy_control_endpoint}")
            InstanceAgent::Log.debug("#{self.class.to_s}: #{e.inspect}")
            false
          end
        end
      end

      class CodeDeployControlCertVerifier
        def initialize(endpoint)
          @endpoint = endpoint
          @region = ENV['AWS_REGION'] || InstanceMetadata.region
        end

        def verify_cert
          uri = URI(@endpoint)
          client = Net::HTTP.new(uri.host, uri.port)
          client.use_ssl = true
          client.verify_mode = OpenSSL::SSL::VERIFY_PEER
          client.ca_file = ENV['SSL_CERT_FILE']

          if InstanceAgent::Config.config[:proxy_uri]
            proxy_uri = URI(InstanceAgent::Config.config[:proxy_uri])
            client.proxy_from_env = false # make sure proxy settings can be overridden
            client.proxy_address = proxy_uri.host
            client.proxy_port = proxy_uri.port
            client.proxy_user = proxy_uri.user if proxy_uri.user
            client.proxy_pass = proxy_uri.password if proxy_uri.password 
          end

          client.verify_callback = lambda do |preverify_ok, cert_store|
            return false unless preverify_ok
            @cert = cert_store.chain[0]
            verify_subject
          end

          response = client.get '/'
        end

        # Do minimal cert pinning
        def verify_subject
          InstanceAgent::Log.debug("#{self.class.to_s}: Actual certificate subject is '#{@cert.subject.to_s}'")
          @cert.subject.to_s == "/C=US/ST=Washington/L=Seattle/O=Amazon.com, Inc./CN=codedeploy-commands."+@region+".amazonaws.com"
        end

      end
    end
  end
end
