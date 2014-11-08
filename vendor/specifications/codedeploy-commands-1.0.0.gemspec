# -*- encoding: utf-8 -*-

Gem::Specification.new do |s|
  s.name = "codedeploy-commands"
  s.version = "1.0.0"

  s.required_rubygems_version = Gem::Requirement.new(">= 0") if s.respond_to? :required_rubygems_version=
  s.authors = ["Amazon Web Services"]
  s.date = "2014-11-04"
  s.description = "Provides client libraries for CodeDeploy Command."
  s.files = ["lib/aws/codedeploy_commands.rb", "lib/aws/plugins/deploy_control_endpoint.rb", "lib/aws/plugins/certificate_authority.rb", "apis/CodeDeployCommand.api.json"]
  s.licenses = ["Apache 2.0"]
  s.require_paths = ["lib"]
  s.rubygems_version = "1.8.23"
  s.summary = "Deploy Control Ruby SDK"

  if s.respond_to? :specification_version then
    s.specification_version = 3

    if Gem::Version.new(Gem::VERSION) >= Gem::Version.new('1.2.0') then
      s.add_runtime_dependency(%q<aws-sdk-core>, ["= 2.0.5"])
    else
      s.add_dependency(%q<aws-sdk-core>, ["= 2.0.5"])
    end
  else
    s.add_dependency(%q<aws-sdk-core>, ["= 2.0.5"])
  end
end
