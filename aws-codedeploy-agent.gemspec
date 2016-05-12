# -*- encoding: utf-8 -*-
$:.push File.expand_path("../lib", __FILE__)
$:.push File.expand_path("../vendor/gems/codedeploy-commands/lib", __FILE__)
$:.push File.expand_path("../vendor/gems/process_manager/lib", __FILE__)

Gem::Specification.new do |s|
  s.name          = 'aws-codedeploy-agent'
  s.version       = '0.1.0'
  s.license       = 'Apache-2.0'
  s.summary       = 'AWS CodeDeploy Agent'
  s.description   = 'CodeDeploy Agent is responsible for doing the actual work of deploying software on an individual EC2 instance.'
  s.authors       = ['Amazon Web Services', 'Pan Thomakos']
  s.email         = ['pan.thomakos@gmail.com']
  s.homepage      = 'https://github.com/panthomakos/aws-codedeploy-agent'
  s.files         = `git ls-files`.split($/)
  s.require_paths = [
    'lib',
    'vendor/gems/codedeploy-commands/lib',
    'vendor/gems/process_manager/lib',
  ]
  s.executables   = s.files.grep(%r{^bin/}) { |f| File.basename(f) }
  s.test_files    = s.files.grep(%r{^(test|spec|features)/})

  s.add_dependency('gli', '~> 2.5')
  s.add_dependency('json_pure', '~> 1.6')
  s.add_dependency('archive-tar-minitar', '~> 0.5.2')
  s.add_dependency('rubyzip', '~> 1.1.0')
  s.add_dependency('rake', '~> 0.9')
  s.add_dependency('logging', '~> 1.8')
  s.add_dependency('aws-sdk-core', '~> 2.2.0')
  s.add_dependency('simple_pid', '~> 0.2.1')
end
