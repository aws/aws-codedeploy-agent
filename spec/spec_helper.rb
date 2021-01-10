# Encoding: UTF-8
# See http://rubydoc.info/gems/rspec-core/RSpec/Core/Configuration
require 'bundler/setup'
require 'webmock/rspec'
Bundler.setup

RSpec.configure do |config|
  config.run_all_when_everything_filtered = true
  config.filter_run :focus
end
