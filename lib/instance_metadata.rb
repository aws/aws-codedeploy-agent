require 'net/http'
require 'json'
require 'instance_agent'

# https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/instance-identity-documents.html
class InstanceMetadata

  IP_ADDRESS = '169.254.169.254'
  PORT = 80
  HTTP_TIMEOUT = 30

  PARTITION_PATH = '/latest/meta-data/services/partition'
  INSTANCE_ID_PATH = '/latest/meta-data/instance-id'
  TOKEN_PATH = '/latest/api/token'
  DOCUMENT_PATH = '/latest/dynamic/instance-identity/document'

  def self.host_identifier
    "arn:#{partition}:ec2:#{doc['region']}:#{doc['accountId']}:instance/#{doc['instanceId']}"
  end

  def self.partition
    get_metadata_wrapper(PARTITION_PATH).strip
  end

  def self.region
    doc['region'].strip
  end

  def self.instance_id
    begin
      get_metadata_wrapper(INSTANCE_ID_PATH)
    rescue
      return nil
    end
  end

  class InstanceMetadataError < StandardError
  end

  private
  def self.get_metadata_wrapper(path)
    begin
      token = put_request(TOKEN_PATH)
      get_request(path, token)
    rescue
      InstanceAgent::Log.send(:info, "IMDSv2 http request failed, falling back to IMDSv1.")
      get_request(path)
    end

  end

  def self.http_request(request)
    Net::HTTP.start(IP_ADDRESS, PORT, :read_timeout => 120, :open_timeout => 120) do |http|
      response = http.request(request)
      if response.code.to_i != 200
        raise "HTTP error from metadata service: #{response.message}, code #{response.code}"
      end
      return response.body
    end
  end

  def self.put_request(path)
    request = Net::HTTP::Put.new(path)
    request['X-aws-ec2-metadata-token-ttl-seconds'] = '21600'
    http_request(request)
  end

  def self.get_request(path, token = nil)
    request = Net::HTTP::Get.new(path)
    unless token.nil?
      request['X-aws-ec2-metadata-token'] = token
    end
    http_request(request)
  end

  def self.doc
    begin
      token = put_request(TOKEN_PATH)
      JSON.parse(get_request(DOCUMENT_PATH, token).strip)
    rescue
      InstanceAgent::Log.send(:info, "IMDSv2 http request failed, falling back to IMDSv1.")
      JSON.parse(get_request(DOCUMENT_PATH).strip)
    end
  end
end
