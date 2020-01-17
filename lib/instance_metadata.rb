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
    http_get('/latest/meta-data/services/partition').strip
  end

  def self.region
    doc['region']
  end

  def self.instance_id
    begin
      Net::HTTP.start(IP_ADDRESS, PORT) do |http|
        response = http.get('/latest/meta-data/instance-id')
        if response.code.to_i != 200
          return nil
        end
        return response.body
      end
    rescue
      return nil
    end
  end

  class InstanceMetadataError < StandardError
  end

  private
  def self.http_get(path)
    Net::HTTP.start(IP_ADDRESS, PORT, :read_timeout => HTTP_TIMEOUT/2, :open_timeout => HTTP_TIMEOUT/2) do |http|
      response = http.get(path)
      if response.code.to_i != 200
        InstanceAgent::Log.send(:debug, "HTTP error from metadata service, code #{response.code}")
        raise "HTTP error from metadata service, code #{response.code}"
      end
      return response.body
    end
  end

  private
  def self.doc
    JSON.parse(http_get('/latest/dynamic/instance-identity/document').strip)
  end
end
