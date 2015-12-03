require 'singleton'

class AwsCredentials
  include Singleton
  attr_reader :ec2_ami
  attr_reader :keypair_name

  def configure
    file_path = File.join(File.expand_path(File.dirname(__FILE__)), './AwsCredentials.yml')
    file_config = nil
    if File.exists?(file_path) && File.readable?(file_path)
      begin
        file_config = YAML.load(File.read(file_path)).inject({}){|temp,(k,v)| temp[k.to_sym] = v; temp}
      rescue Exception => e
        puts("Invalid AwsCredentials file")
        raise "Invalid AwsCredentials file"
      end
    else
      puts 'AwsCredentials.yml file does not exist'
    end
    if file_config
      if file_config[:region]
        ENV['AWS_REGION'] = file_config[:region]
      end
      if file_config[:aws_access_key_id]
        ENV['AWS_ACCESS_KEY_ID'] = file_config[:aws_access_key_id]
      end
      if file_config[:aws_secret_access_key]
        ENV['AWS_SECRET_ACCESS_KEY'] = file_config[:aws_secret_access_key]
      end
      @ec2_ami ||= file_config[:ec2_ami_id]
      @keypair_name ||= file_config[:keypair_name]
    end
  end
end
