require 'simplecov'

SimpleCov.start do
  if ENV['CI']
    require 'simplecov-lcov'

    SimpleCov::Formatter::LcovFormatter.config do |c|
      c.report_with_single_file = true
      c.single_report_path = 'coverage/lcov.info'
    end

    formatter SimpleCov::Formatter::LcovFormatter
  end

  add_filter %w[version.rb initializer.rb]
end

# Encoding: UTF-8
# See http://rubydoc.info/gems/rspec-core/RSpec/Core/Configuration
require 'bundler/setup'
require 'webmock/rspec'
Bundler.setup

RSpec.configure do |config|
  config.run_all_when_everything_filtered = true
  config.filter_run :focus
end
