require 'net/http'
require 'json'
require 'instance_agent'

class InstanceMetadata

  IP_ADDRESS = '169.254.169.254'
  PORT = 80
  
  def self.host_identifier
    doc = JSON.parse(http_get('/latest/dynamic/instance-identity/document').strip)
    "arn:aws:ec2:#{doc['region']}:#{doc['accountId']}:instance/#{doc['instanceId']}"
  end

  def self.region
    az = http_get('/latest/meta-data/placement/availability-zone').strip
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

  private
  def self.http_get(path)
    Net::HTTP.start(IP_ADDRESS, PORT) do |http|
      response = http.get(path)
      if response.code.to_i != 200
        InstanceAgent::Log.send(:debug, "HTTP error from metadata service, code #{response.code}")
        raise "HTTP error from metadata service, code #{response.code}"
      end
      return response.body
    end
  end
end
