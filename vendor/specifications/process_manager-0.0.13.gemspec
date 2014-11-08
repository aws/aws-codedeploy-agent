# -*- encoding: utf-8 -*-

Gem::Specification.new do |s|
  s.name = "process_manager"
  s.version = "0.0.13"

  s.required_rubygems_version = Gem::Requirement.new(">= 0") if s.respond_to? :required_rubygems_version=
  s.authors = ["Jonathan Weiss"]
  s.date = "2013-01-31"
  s.description = "A framework to manage processes using one master and many children processes"
  s.email = "info@peritor.com"
  s.extra_rdoc_files = ["README.md"]
  s.files = ["README.md"]
  s.homepage = "http://github.com/scalarium/process_manager"
  s.require_paths = ["lib"]
  s.rubygems_version = "1.8.23"
  s.summary = "A framework to manage processes"

  if s.respond_to? :specification_version then
    s.specification_version = 3

    if Gem::Version.new(Gem::VERSION) >= Gem::Version.new('1.2.0') then
      s.add_runtime_dependency(%q<rake>, [">= 0"])
      s.add_runtime_dependency(%q<simple_pid>, [">= 0"])
      s.add_runtime_dependency(%q<gli>, [">= 2.2.1"])
      s.add_runtime_dependency(%q<simple_pid>, [">= 0"])
      s.add_runtime_dependency(%q<gli>, [">= 0"])
    else
      s.add_dependency(%q<rake>, [">= 0"])
      s.add_dependency(%q<simple_pid>, [">= 0"])
      s.add_dependency(%q<gli>, [">= 2.2.1"])
      s.add_dependency(%q<simple_pid>, [">= 0"])
      s.add_dependency(%q<gli>, [">= 0"])
    end
  else
    s.add_dependency(%q<rake>, [">= 0"])
    s.add_dependency(%q<simple_pid>, [">= 0"])
    s.add_dependency(%q<gli>, [">= 2.2.1"])
    s.add_dependency(%q<simple_pid>, [">= 0"])
    s.add_dependency(%q<gli>, [">= 0"])
  end
end
