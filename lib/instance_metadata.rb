require 'net/http'
require 'json'
require 'instance_agent'

# InstanceMetadata provides access to IMDS V1 and V2. When able, InstanceMetadata will use 
# IMDS v2 but fall back to v1 when not. Should both fail, any property inquary will be nil.
# More info about IMDS can be found at:
# https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/instance-identity-documents.html
class InstanceMetadata

  IP_ADDRESS = '169.254.169.254'
  PORT = 80
  # "The IP address 169.254.169.254 is a link-local address and is valid only from the instance." Hence why
  # the low timeout
  HTTP_TIMEOUT = 10

  BASE_PATH = '/latest/meta-data'
  PARTITION_PATH = '/latest/meta-data/services/partition'
  DOMAIN_PATH = '/latest/meta-data/services/domain'
  INSTANCE_ID_PATH = '/latest/meta-data/instance-id'
  TOKEN_PATH = '/latest/api/token'
  IDENTITY_DOCUMENT_PATH = '/latest/dynamic/instance-identity/document'

  def self.imds_supported?
    imds_v2? || imds_v1?
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
      put_request(TOKEN_PATH) { |token_response|
        (token_response.kind_of? Net::HTTPSuccess) && get_request(BASE_PATH, token_response.body) { |response|
          return response.kind_of? Net::HTTPSuccess
        }
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
      token = put_request(TOKEN_PATH)
      get_request(path, token)
    rescue
      InstanceAgent::Log.send(:info, "IMDSv2 http request failed, falling back to IMDSv1.")
      get_request(path)
    end
  end

  private
  def self.http_request(request)
    Net::HTTP.start(IP_ADDRESS, PORT, :read_timeout => HTTP_TIMEOUT, :open_timeout => HTTP_TIMEOUT) do |http|
      response = http.request(request)
      if block_given?
        yield(response)
      elsif response.kind_of? Net::HTTPSuccess
        response.body
      else
        raise "HTTP error from metadata service: #{response.message}, code #{response.code}"
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
    http_request(request, &block)
  end
end
