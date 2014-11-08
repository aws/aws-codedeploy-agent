gem_root = File.dirname(File.dirname(File.dirname(__FILE__)))

require 'aws-sdk-core'
require "#{gem_root}/lib/aws/plugins/certificate_authority"
require "#{gem_root}/lib/aws/plugins/deploy_control_endpoint"

version = '1.0.0'

bundled_apis = Dir.glob(File.join(gem_root, 'apis', '*.json')).group_by do |path|
  File.basename(path).split('.').first
end

bundled_apis.each do |svc_class_name, api_versions|
  svc_class = Aws.add_service(svc_class_name, api: JSON.parse(File.read(api_versions.first), max_nesting: false))
  svc_class.const_set(:VERSION, version)
  Aws::CodeDeployCommand::Client.add_plugin(Aws::Plugins::CertificateAuthority)
  Aws::CodeDeployCommand::Client.add_plugin(Aws::Plugins::DeployControlEndpoint)
end
