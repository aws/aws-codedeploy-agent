task :default => :test

require 'rake/testtask'
Rake::TestTask.new do |t|
  t.libs.push 'test'
  t.pattern = 'test/**/*_test.rb'
  t.verbose = true
end
