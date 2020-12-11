require 'aws-sdk-core'

module Aws
  module Plugins
    class DeployControlEndpoint < Seahorse::Client::Plugin
      option(:endpoint) do |cfg|

        # Allow the customer to manually configure the endpoint
        url = InstanceAgent::Config.config[:deploy_control_endpoint]

        if url.nil?
          service = 'codedeploy-commands'
          if InstanceAgent::Config.config[:enable_auth_policy]
            service += '-secure'
          end
          if InstanceAgent::Config.config[:use_fips_mode]
            service += '-fips'
          end

          if InstanceMetadata.imds_supported?
            region = InstanceMetadata.region
            domain = InstanceMetadata.domain
          else
            region = cfg.region
            domain = 'amazonaws.com'
            domain += '.cn' if region.split("-")[0] == 'cn'
          end

          url = "https://#{service}.#{region}.#{domain}"
        end

        ProcessManager::Log.info("CodeDeploy endpoint: #{url}")
        url
      end
    end
  end
end
