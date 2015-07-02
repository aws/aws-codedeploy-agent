# -*- encoding: utf-8 -*-

Gem::Specification.new do |s|
  s.name = "gli"
  s.version = "2.5.6"

  s.required_rubygems_version = Gem::Requirement.new(">= 0") if s.respond_to? :required_rubygems_version=
  s.authors = ["David Copeland"]
  s.date = "2013-03-31"
  s.description = "Build command-suite CLI apps that are awesome.  Bootstrap your app, add commands, options and documentation while maintaining a well-tested idiomatic command-line app"
  s.email = "davidcopeland@naildrivin5.com"
  s.executables = ["gli"]
  s.extra_rdoc_files = ["README.rdoc", "gli.rdoc"]
  s.files = ["bin/gli", "README.rdoc", "gli.rdoc"]
  s.homepage = "http://davetron5000.github.com/gli"
  s.rdoc_options = ["--title", "Git Like Interface", "--main", "README.rdoc"]
  s.require_paths = ["lib"]
  s.rubyforge_project = "gli"
  s.rubygems_version = "1.8.23.2"
  s.summary = "Build command-suite CLI apps that are awesome."

  if s.respond_to? :specification_version then
    s.specification_version = 3

    if Gem::Version.new(Gem::VERSION) >= Gem::Version.new('1.2.0') then
      s.add_development_dependency(%q<rake>, ["~> 0.9.2.2"])
      s.add_development_dependency(%q<rdoc>, ["~> 3.11"])
      s.add_development_dependency(%q<rainbow>, ["~> 1.1.1"])
      s.add_development_dependency(%q<clean_test>, [">= 0"])
      s.add_development_dependency(%q<aruba>, [">= 0"])
      s.add_development_dependency(%q<sdoc>, [">= 0"])
      s.add_development_dependency(%q<faker>, ["= 1.0.0"])
    else
      s.add_dependency(%q<rake>, ["~> 0.9.2.2"])
      s.add_dependency(%q<rdoc>, ["~> 3.11"])
      s.add_dependency(%q<rainbow>, ["~> 1.1.1"])
      s.add_dependency(%q<clean_test>, [">= 0"])
      s.add_dependency(%q<aruba>, [">= 0"])
      s.add_dependency(%q<sdoc>, [">= 0"])
      s.add_dependency(%q<faker>, ["= 1.0.0"])
    end
  else
    s.add_dependency(%q<rake>, ["~> 0.9.2.2"])
    s.add_dependency(%q<rdoc>, ["~> 3.11"])
    s.add_dependency(%q<rainbow>, ["~> 1.1.1"])
    s.add_dependency(%q<clean_test>, [">= 0"])
    s.add_dependency(%q<aruba>, [">= 0"])
    s.add_dependency(%q<sdoc>, [">= 0"])
    s.add_dependency(%q<faker>, ["= 1.0.0"])
  end
end
