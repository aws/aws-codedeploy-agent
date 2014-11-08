require File.join(File.dirname(__FILE__), 'lib', 'jamespath', 'version')

Gem::Specification.new do |spec|
  spec.name          = 'jamespath'
  spec.version       = Jamespath::VERSION
  spec.summary       = 'Implements JMESpath declarative object searching.'
  spec.description   = 'Like XPath, but for JSON and other structured objects.'
  spec.authors       = ['Loren Segal', 'Trevor Rowe']
  spec.email         = 'lsegal@soen.ca'
  spec.homepage      = 'http://github.com/lsegal/jamespath'
  spec.license       = 'MIT'
  spec.files         = `git ls-files`.split($/)
  spec.test_files    = spec.files.grep(%r{^test/})
  spec.add_development_dependency('rake', '~> 10.0')
  spec.add_development_dependency('yard', '~> 0.0')
  spec.add_development_dependency('rdiscount', '>= 2.1.7', '< 3.0')
end
