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
      @credentials = Aws::SharedCredentials.new(path: @path).credentials
      raise "Failed to load credentials from path #{@path}" if @credentials.nil?
      @expiration = Time.new + 1800
    rescue Aws::Errors::NoSuchProfileError
      raise "Failed to load credentials from path #{@path}"
    end
  end
end
