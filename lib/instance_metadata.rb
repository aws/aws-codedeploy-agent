require 'net/http'
require 'json'
require 'instance_agent'

class InstanceMetadata

  IP_ADDRESS = '169.254.169.254'
  PORT = 80
  HTTP_TIMEOUT = 30
  
  def self.host_identifier
    doc = JSON.parse(http_get('/latest/dynamic/instance-identity/document').strip)
    "arn:#{partition}:ec2:#{doc['region']}:#{doc['accountId']}:instance/#{doc['instanceId']}"
  end

  def self.partition
    http_get('/latest/meta-data/services/partition').strip
  end

  def self.region
    begin 
      az = http_get('/latest/meta-data/placement/availability-zone').strip
    rescue Net::ReadTimeout, Net::OpenTimeout
      raise InstanceMetadata::InstanceMetadataError.new('Not an EC2 instance and region not provided in the environment variable AWS_REGION. Please specify your region using environment variable AWS_REGION.')
    end

    raise "Invalid availability zone name: #{az}" unless
      az =~ /[a-z]{2}-[a-z]+-\d+[a-z]/
    az.chop
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
end
