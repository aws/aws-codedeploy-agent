require 'net/http'
require 'json'
require 'instance_agent'

# https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/instance-identity-documents.html
class InstanceMetadata

  IP_ADDRESS = '169.254.169.254'
  PORT = 80
  HTTP_TIMEOUT = 30

  def self.host_identifier
    "arn:#{partition}:ec2:#{doc['region']}:#{doc['accountId']}:instance/#{doc['instanceId']}"
  end

  def self.partition
    get_metadata_wrapper('/latest/meta-data/services/partition').strip
  end

  def self.region
    doc['region'].strip
  end

  def self.instance_id
    begin
      get_metadata_wrapper('/latest/meta-data/instance-id')
    rescue
      return nil
    end
  end

  class InstanceMetadataError < StandardError
  end

  private
  def self.get_metadata_wrapper(path)
    token = put_request('/latest/api/token')
    get_request(path, token)
  end

  def self.http_request(request)
    Net::HTTP.start('169.254.169.254', 80, :read_timeout => 120, :open_timeout => 120) do |http|
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

  def self.get_request(path, token)
    request = Net::HTTP::Get.new(path)
    request['X-aws-ec2-metadata-token'] = token
    http_request(request)
  end

  def self.doc
    token = put_request('/latest/api/token')
    JSON.parse(get_request('/latest/dynamic/instance-identity/document', token).strip)
  end
end
