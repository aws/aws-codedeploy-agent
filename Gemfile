
# we do not want any user gems,
# only the ones bundled by us
#disable_system_gems

source 'http://rubygems.org'
# our dependencies
gem 'json_pure'
gem 'gli'
gem 'aws-sdk-core'
gem 'codedeploy-commands'
gem 'rubyzip'
gem 'rake'
gem 'archive-tar-minitar'
gem 'logging'

group :development do
# this doesn't need to be a global or even a standard dependency
# use it if you need it, but don't commit it.
#  gem 'ruby-debug', :require => nil
#  gem 'ruby-debug-base', :require => nil
end

group :test do
  gem 'test-unit'
  gem 'shoulda'
  gem 'shoulda-matchers'
  gem 'shoulda-context'
  gem 'mocha'
  gem 'fakefs', :require => 'fakefs/safe'
  gem 'activesupport', :require => 'active_support'
end
