Gem::Specification.new do |spec|
  spec.name          = 'aws_codedeploy_agent'
  spec.version       = '1.4.1'
  spec.summary       = 'Packages AWS CodeDeploy agent libraries'
  spec.description   = 'AWS CodeDeploy agent is responsible for doing the actual work of deploying software on an individual EC2 instance'
  spec.author        = 'Amazon Web Services'
  spec.files         = Dir['{lib,bin,conf,vendor}/**/*']
  spec.homepage      = "https://github.com/aws/aws-codedeploy-agent"
  spec.bindir        = ['bin']
  spec.require_paths = ['lib']
  spec.license        = 'Apache-2.0'
  spec.required_ruby_version = '~> 2.0'

  spec.add_dependency('gli', '~> 2.5')
  spec.add_dependency('json_pure', '~> 1.6')
  spec.add_dependency('archive-tar-minitar', '~> 0.5.2')
  spec.add_dependency('rubyzip', '~> 1.3.0')
  spec.add_dependency('logging', '~> 1.8')
  spec.add_dependency('aws-sdk-core', '~> 3')
  spec.add_dependency('aws-sdk-code-generator', '~> 0.2.2.pre')
  spec.add_dependency('aws-sdk-s3', '~> 1')
  spec.add_dependency('docopt', '~> 0.5.0')
  spec.add_dependency('concurrent-ruby', '~> 1.1.9')

  spec.add_development_dependency('rake', '~> 12.3.3')
  spec.add_development_dependency('rspec', '~> 3.2.0')
end
