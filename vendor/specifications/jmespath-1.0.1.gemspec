# -*- encoding: utf-8 -*-

Gem::Specification.new do |s|
  s.name = "jmespath"
  s.version = "1.0.1"

  s.required_rubygems_version = Gem::Requirement.new(">= 0") if s.respond_to? :required_rubygems_version=
  s.authors = ["Trevor Rowe"]
  s.date = "2014-10-28"
  s.description = "Implementes JMESPath for Ruby"
  s.email = "trevorrowe@gmail.com"
  s.homepage = "http://github.com/trevorrowe/jmespath.rb"
  s.licenses = ["Apache 2.0"]
  s.require_paths = ["lib"]
  s.rubygems_version = "1.8.23.2"
  s.summary = "JMESPath - Ruby Edition"

  if s.respond_to? :specification_version then
    s.specification_version = 4

    if Gem::Version.new(Gem::VERSION) >= Gem::Version.new('1.2.0') then
      s.add_runtime_dependency(%q<multi_json>, ["~> 1.0"])
    else
      s.add_dependency(%q<multi_json>, ["~> 1.0"])
    end
  else
    s.add_dependency(%q<multi_json>, ["~> 1.0"])
  end
end
