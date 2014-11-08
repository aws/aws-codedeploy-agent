# -*- encoding: utf-8 -*-

Gem::Specification.new do |s|
  s.name = "jamespath"
  s.version = "0.5.1"

  s.required_rubygems_version = Gem::Requirement.new(">= 0") if s.respond_to? :required_rubygems_version=
  s.authors = ["Loren Segal", "Trevor Rowe"]
  s.date = "2014-09-11"
  s.description = "Like XPath, but for JSON and other structured objects."
  s.email = "lsegal@soen.ca"
  s.homepage = "http://github.com/lsegal/jamespath"
  s.licenses = ["MIT"]
  s.require_paths = ["lib"]
  s.rubygems_version = "1.8.23"
  s.summary = "Implements JMESpath declarative object searching."

  if s.respond_to? :specification_version then
    s.specification_version = 4

    if Gem::Version.new(Gem::VERSION) >= Gem::Version.new('1.2.0') then
      s.add_development_dependency(%q<rake>, ["~> 10.0"])
      s.add_development_dependency(%q<yard>, ["~> 0.0"])
      s.add_development_dependency(%q<rdiscount>, ["< 3.0", ">= 2.1.7"])
    else
      s.add_dependency(%q<rake>, ["~> 10.0"])
      s.add_dependency(%q<yard>, ["~> 0.0"])
      s.add_dependency(%q<rdiscount>, ["< 3.0", ">= 2.1.7"])
    end
  else
    s.add_dependency(%q<rake>, ["~> 10.0"])
    s.add_dependency(%q<yard>, ["~> 0.0"])
    s.add_dependency(%q<rdiscount>, ["< 3.0", ">= 2.1.7"])
  end
end
