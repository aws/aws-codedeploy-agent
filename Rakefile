require 'rake'
require 'rake/testtask'
require 'rubygems'

# Run all units tests in test/
desc "Run unit tests in test/"
Rake::TestTask.new(:test) do |t|
  t.libs << ['test', 'lib', 'test/helpers']

  test_files = FileList.new("test/**/*_test.rb")
  t.test_files = test_files
  t.verbose = true
end
task :default => :test
task :release => :test

# Run units tests in test/instance_agent/
Rake::TestTask.new(:test_instance_agent) do |t|
  t.libs << ['test', 'lib', 'test/helpers']
  t.pattern = "test/instance_agent/**/*_test.rb"
  t.verbose = true
end

# Clean up
task :clean do
  rm_rf 'deployment'
end
