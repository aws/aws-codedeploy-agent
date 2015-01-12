require 'aws-sdk-core'

module Aws
  module Plugins
    class CertificateAuthority < Seahorse::Client::Plugin

      option(:ssl_ca_bundle)    { ENV['AWS_SSL_CA_BUNDLE'] }
      option(:ssl_ca_directory) { ENV['AWS_SSL_CA_DIRECTORY'] }

    end
  end
end
