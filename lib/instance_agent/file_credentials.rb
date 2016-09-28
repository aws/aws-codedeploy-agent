require 'aws-sdk-core'

module InstanceAgent
  class FileCredentials
    include Aws::CredentialProvider
    include Aws::RefreshingCredentials

    # @param [String] path
    def initialize(path)
      @path = path
      super()
    end

    private

    def refresh
      @credentials = Aws::SharedCredentials.new(path: @path)
      @expiration = Time.new + 1800
    end
  end
end
