# we do not want any user gems,
# only the ones bundled by us
#disable_system_gems

source 'http://rubygems.org'

gemspec
gem "process_manager", "0.0.13", :path => "#{File.expand_path(__FILE__)}/../vendor/gems/process_manager-0.0.13"
gem "codedeploy-commands", "1.0.0", :path => "#{File.expand_path(__FILE__)}/../vendor/gems/codedeploy-commands-1.0.0"
gem "simple_pid", "0.2.1", :path => "#{File.expand_path(__FILE__)}/../vendor/gems/simple_pid-0.2.1"

group :test do
  gem 'test-unit'
  gem 'activesupport', :require => 'active_support'
  gem 'coveralls_reborn', require: false
  gem 'cucumber'
  gem 'fakefs', :require => 'fakefs/safe'
  gem 'mocha', "0.13.3"
  gem 'rspec'
  gem 'webmock', :require => 'webmock/rspec'
  gem 'shoulda'
  gem 'shoulda-matchers'
  gem 'shoulda-context'
  gem 'simplecov', require: false
  gem 'simplecov-lcov', require: false
end
