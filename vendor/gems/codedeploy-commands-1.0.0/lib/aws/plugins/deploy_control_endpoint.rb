require 'aws-sdk-core'

module Aws
  module Plugins
    class DeployControlEndpoint < Seahorse::Client::Plugin
      option(:endpoint) do |cfg|
        url = ENV['AWS_DEPLOY_CONTROL_ENDPOINT']
        if url.nil?
          case cfg.region
            when "us-east-1"
              url = "https://codedeploy-commands.us-east-1.amazonaws.com"
            when "us-west-2"
              url = "https://codedeploy-commands.us-west-2.amazonaws.com"
            else
              raise "Not able to find an endpoint. Unknown region."
          end
        end
        url
      end
    end
  end
end
