require 'aws-sdk-core'

module Aws
  module Plugins
    class DeployControlEndpoint < Seahorse::Client::Plugin
      option(:endpoint) do |cfg|
        service = 'codedeploy-commands'
        region = InstanceMetadata.region
        domain = InstanceMetadata.domain
        url = InstanceAgent::Config.config[:deploy_control_endpoint]
        if url.nil?
          if InstanceAgent::Config.config[:enable_auth_policy]
            service += '-secure'
          end
          if InstanceAgent::Config.config[:use_fips_mode]
            service += '-fips'
          end
          url = "https://#{service}.#{region}.#{domain}"
          ProcessManager::Log.info("ADCS endpoint: #{url}")
        end
        url
      end
    end
  end
end
