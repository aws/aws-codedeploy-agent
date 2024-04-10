require 'net/http'
require 'json'
require 'instance_agent'

# InstanceMetadata provides access to IMDS V1 and V2. When disable IMDS V1, When enable InstanceMetadata will
# always use IMDS v2 and fail the request when IMDSv2 is not available. When enable IMDS V1, InstanceMetadata
# will use IMDS v2 but fall back to v1 when not, when token query got issue. Should both fail, any property inquary will be nil.
# More info about IMDS can be found at:
# https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/instance-identity-documents.html
class InstanceMetadata

  IP_ADDRESS = '169.254.169.254'
  PORT = 80
  # "The IP address 169.254.169.254 is a link-local address and is valid only from the instance." Hence why
  # the low timeout
  HTTP_TIMEOUT = 10
  HTTP_MAX_RETRY_COUNT = 2

  BASE_PATH = '/latest/meta-data'
  PARTITION_PATH = '/latest/meta-data/services/partition'
  DOMAIN_PATH = '/latest/meta-data/services/domain'
  INSTANCE_ID_PATH = '/latest/meta-data/instance-id'
  TOKEN_PATH = '/latest/api/token'
  IDENTITY_DOCUMENT_PATH = '/latest/dynamic/instance-identity/document'

  def self.imds_supported?
    imds_v2? || imds_v1?
  end

  def self.disable_imds_v1?
    !!ProcessManager::Config.config[:disable_imds_v1]
  end

  def self.imds_v1?
    begin
      get_request(BASE_PATH) { |response|
        return response.kind_of? Net::HTTPSuccess
      }
    rescue
      false
    end
  end

  def self.host_identifier
    doc = identity_document()
    doc.nil? ? nil : "arn:#{partition()}:ec2:#{doc['region']}:#{doc['accountId']}:instance/#{doc['instanceId']}"
  end

  def self.imds_v2?
    begin
      token = get_imds_v2_token(TOKEN_PATH)
      get_request(BASE_PATH, token) { |response|
        return response.kind_of? Net::HTTPSuccess
      }
    rescue
      false
    end
  end

  def self.partition
    begin
      get_instance_metadata(PARTITION_PATH).strip
    rescue
      nil
    end
  end

  def self.domain
    begin
      get_instance_metadata(DOMAIN_PATH).strip
    rescue
      nil
    end
  end

  def self.instance_id
    begin
      get_instance_metadata(INSTANCE_ID_PATH).strip
    rescue
      nil
    end
  end

  def self.region
    begin
      identity_document()['region'].strip
    rescue
      nil
    end
  end

  def self.identity_document
    JSON.parse(get_instance_metadata(IDENTITY_DOCUMENT_PATH).strip)
  end

  private
  def self.get_instance_metadata(path)
    begin
      token = get_imds_v2_token(TOKEN_PATH)
    rescue
      if disable_imds_v1?
        raise "HTTP error from metadata service to get imdsv2 token."
      end
      InstanceAgent::Log.send(:warn, "IMDSv2 http request failed, falling back to IMDSv1.")
      return get_request(path)
    end
    get_request(path, token)
  end

  private
  def self.get_imds_v2_token(path)
    @@current_imds_v2_token ||= put_request(path)
  end

  private
  def self.http_request(request)
    retry_interval_in_sec = [1, 2]
    begin
      Net::HTTP.start(IP_ADDRESS, PORT, :read_timeout => HTTP_TIMEOUT, :open_timeout => HTTP_TIMEOUT, :max_retries => HTTP_MAX_RETRY_COUNT) do |http|
        response = http.request(request)
        if block_given?
          yield(response)
        elsif response.kind_of? Net::HTTPSuccess
          response.body
        elsif response.kind_of? Net::HTTPUnauthorized
          # 401 Error
          raise HTTPUnauthorizedError.new("HTTP error from metadata service: #{response.message}, code #{response.code}")
        else
          raise "HTTP error from metadata service: #{response.message}, code #{response.code}"
        end
      end
    rescue
      if delay = retry_interval_in_sec.shift # will be nil if the list is empty
        sleep delay
        retry # backs up to just after the "begin"
      else
        raise # with no args re-raises original error
      end
    end
  end

  private
  def self.put_request(path, &block)
    request = Net::HTTP::Put.new(path)
    request['X-aws-ec2-metadata-token-ttl-seconds'] = '21600'
    http_request(request, &block)
  end

  private
  def self.get_request(path, token = nil, &block)
    request = Net::HTTP::Get.new(path)
    unless token.nil?
      request['X-aws-ec2-metadata-token'] = token
    end
    begin
      http_request(request, &block)
    rescue HTTPUnauthorizedError
      unless token.nil?
        @@current_imds_v2_token = nil
        request['X-aws-ec2-metadata-token'] = get_imds_v2_token(TOKEN_PATH)
        return http_request(request, &block)
      end
      raise
    end
  end

  private
  class HTTPUnauthorizedError < StandardError
    def initialize(msg="HTTPUnauthorizedError")
      super
    end
  end
end
