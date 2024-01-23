require 'aws-sdk-core'
require 'json'

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

            if is_on_prem?
              partitions_region_pattern = File.read(File.join(File.dirname(__FILE__), 'partition-region-pattern.json'))
              partitions_region_pattern_hash = JSON.parse(partitions_region_pattern)
              
              unless partitions_region_pattern_hash.include?(domain)
                raise "Unknown domain: #{domain}"
              end
              
              known_region_pattern = partitions_region_pattern_hash[domain]["regionRegex"]
              
              unless region.match(known_region_pattern)
                raise "Invalid region: #{region}"
              end
            end

            ProcessManager::Log.info("Creating client url from IMDS region and domain")
          else
            region = cfg.region
            domain = 'amazonaws.com'
            domain += '.cn' if region.split("-")[0] == 'cn'

            ProcessManager::Log.info("Creating client url from configurations")
          end

          url = "https://#{service}.#{region}.#{domain}"
        end

        ProcessManager::Log.info("CodeDeploy endpoint: #{url}")
        url
      end

      def self.is_on_prem?
        return File.readable?(InstanceAgent::Config.config[:on_premises_config_file])
      end
    end
  end
end
