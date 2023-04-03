gem_root = File.dirname(File.dirname(File.dirname(__FILE__)))

require "#{gem_root}/lib/aws/plugins/certificate_authority"
require "#{gem_root}/lib/aws/plugins/deploy_control_endpoint"
require "#{gem_root}/lib/aws/plugins/deploy_agent_version"

if InstanceAgent::Config.config[:enable_auth_policy]
  require "#{gem_root}/sdks/codedeploy_commands_secure_sdk"
elsif InstanceAgent::Config.config[:use_mock_command_service]
  require "#{gem_root}/sdks/codedeploy_commands_mock_sdk"
else
  require "#{gem_root}/sdks/codedeploy_commands_sdk"
end

Aws::CodeDeployCommand::Client.add_plugin(Aws::Plugins::CertificateAuthority)
Aws::CodeDeployCommand::Client.add_plugin(Aws::Plugins::DeployControlEndpoint)
Aws::CodeDeployCommand::Client.add_plugin(Aws::Plugins::DeployAgentVersion)

