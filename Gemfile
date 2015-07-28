# we do not want any user gems,
# only the ones bundled by us
#disable_system_gems

source 'http://rubygems.org'

gemspec
gem "process_manager", "0.0.13", :path => "#{File.expand_path(__FILE__)}/../vendor/gems/process_manager-0.0.13"
gem "codedeploy-commands", "1.0.0", :path => "#{File.expand_path(__FILE__)}/../vendor/gems/codedeploy-commands-1.0.0"

group :test do
  gem 'test-unit'
  gem 'activesupport', :require => 'active_support'  
  gem 'coveralls', require: false
  gem 'cucumber'
  gem 'fakefs', :require => 'fakefs/safe'
  gem 'mocha'
  gem 'rspec'
  gem 'shoulda'
  gem 'shoulda-matchers'
  gem 'shoulda-context'
end
