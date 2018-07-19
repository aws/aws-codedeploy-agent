require File.expand_path('../lib/process_manager/version', __FILE__)

Gem::Specification.new do |spec|
  spec.name          = 'process_manager'
  spec.version       = ProcessManager::VERSION
  spec.summary       = 'Process Manager'
  spec.files         = Dir['{lib}/**/*']
  spec.require_paths = ['lib']
  spec.author        = 'Amazon Web Services'
  spec.add_dependency('logging', '~> 1.8')
  spec.add_dependency('simple_pid', '~> 0.2.1')
end
