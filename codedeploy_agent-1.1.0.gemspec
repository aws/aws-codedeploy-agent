Gem::Specification.new do |spec|
  spec.name          = 'codedeploy_agent'
  spec.version       = 0.1
  spec.summary       = 'packages codedeploy agent libraries'
  spec.description   = 'codedeploy agent is responsible for doing the actual work of deploying software on an individual EC2 instance'
  spec.author        = 'Amazon Web Services'
  spec.files         = FileList["lib/**/*", "bin/**/*", "conf/**/*", "vendor/**/*"]
  spec.bindir        = ['bin']
  spec.require_paths = ['lib']
  spec.required_ruby_version = '~> 1.9'
  spec.add_dependency('gli', '~> 2.5')
  spec.add_dependency('json_pure', '~> 1.6')
  spec.add_dependency('archive-tar-minitar', '~> 0.5.2')
  spec.add_dependency('rubyzip', '~> 1.1.0')
  spec.add_dependency('rake', '~> 0.9')
  spec.add_dependency('logging', '~>1.8')
  spec.add_dependency('aws-sdk-core', '~>2.0')
end
