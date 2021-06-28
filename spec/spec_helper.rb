require 'simplecov'
require 'simplecov-lcov'

SimpleCov::Formatter::LcovFormatter.config do |c|
  c.report_with_single_file = true
  c.single_report_path = 'coverage/lcov.info'
end
SimpleCov.formatters = SimpleCov::Formatter::MultiFormatter.new(
  [
    SimpleCov::Formatter::HTMLFormatter,
    SimpleCov::Formatter::LcovFormatter,
  ]
)
SimpleCov.start

# Encoding: UTF-8
# See http://rubydoc.info/gems/rspec-core/RSpec/Core/Configuration
require 'bundler/setup'
require 'webmock/rspec'
Bundler.setup

RSpec.configure do |config|
  config.run_all_when_everything_filtered = true
  config.filter_run :focus
end
