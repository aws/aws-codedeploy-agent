require 'instance_metadata'

module InstanceAgent
  module CodeDeployPlugin
    class RequestHelper

      def initialize(options = {})
        @deploy_control_client = options[:deploy_control_client]
      end

      def verify_clients_configuration
        errors = []
        errors << "Invalid aws sdk security configuration" unless valid_aws_sdk_security_config?
        errors << "Invalid server certificate" unless valid_server_certificate?
        errors
      end
 
      def valid_aws_sdk_security_config?
        @deploy_control_client.ssl_verify_peer
      end

      def valid_server_certificate?
        @deploy_control_client.verify_cert_fields
      end

    end
  end
end
