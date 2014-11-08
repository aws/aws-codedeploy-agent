# -*- encoding: utf-8 -*-

Gem::Specification.new do |s|
  s.name = "aws-sdk-core"
  s.version = "2.0.5"

  s.required_rubygems_version = Gem::Requirement.new(">= 0") if s.respond_to? :required_rubygems_version=
  s.authors = ["Amazon Web Services"]
  s.date = "2014-10-23"
  s.description = "Provides API clients for AWS."
  s.executables = ["aws.rb"]
  s.files = ["bin/aws.rb"]
  s.homepage = "http://github.com/aws/aws-sdk-core-ruby"
  s.licenses = ["Apache 2.0"]
  s.require_paths = ["lib"]
  s.rubygems_version = "1.8.23"
  s.summary = "AWS SDK for Ruby - Core"

  if s.respond_to? :specification_version then
    s.specification_version = 4

    if Gem::Version.new(Gem::VERSION) >= Gem::Version.new('1.2.0') then
      s.add_runtime_dependency(%q<multi_json>, ["~> 1.0"])
      s.add_runtime_dependency(%q<multi_xml>, ["~> 0.5"])
      s.add_runtime_dependency(%q<builder>, ["~> 3.0"])
      s.add_runtime_dependency(%q<jamespath>, [">= 0.5.1"])
    else
      s.add_dependency(%q<multi_json>, ["~> 1.0"])
      s.add_dependency(%q<multi_xml>, ["~> 0.5"])
      s.add_dependency(%q<builder>, ["~> 3.0"])
      s.add_dependency(%q<jamespath>, [">= 0.5.1"])
    end
  else
    s.add_dependency(%q<multi_json>, ["~> 1.0"])
    s.add_dependency(%q<multi_xml>, ["~> 0.5"])
    s.add_dependency(%q<builder>, ["~> 3.0"])
    s.add_dependency(%q<jamespath>, [">= 0.5.1"])
  end
end
