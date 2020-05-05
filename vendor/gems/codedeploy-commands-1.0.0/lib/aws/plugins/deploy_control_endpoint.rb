require 'aws-sdk-core'

module Aws
  module Plugins
    class DeployControlEndpoint < Seahorse::Client::Plugin
      option(:endpoint) do |cfg|
        url = InstanceAgent::Config.config[:deploy_control_endpoint]
        if url.nil?
          url = "https://codedeploy-commands"
          if InstanceAgent::Config.config[:enable_auth_policy]
            url.concat "-secure"
          end
          if InstanceAgent::Config.config[:use_fips_mode]
            url.concat "-fips"
          end
          url.concat ".#{cfg.region}.amazonaws.com"
          if "cn" == cfg.region.split("-")[0]
            url.concat(".cn")
          end
        end
        url
      end
    end
  end
end
