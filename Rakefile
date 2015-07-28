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

begin
  require 'cucumber'
  require 'cucumber/rake/task'
  desc = 'aws codedeploy agent integration tests'
  Cucumber::Rake::Task.new('test-integration-aws-codedeploy-agent', desc) do |t|
    t.cucumber_opts = "features -t ~@Ignore"
  end
  task 'test-integration' => 'test-integration-aws-codedeploy-agent'
rescue LoadError
  desc 'aws codedeploy agent integration tests'
  task 'test:integration' do
    puts 'skipping aws-codedeploy-agent integration tests, cucumber not loaded'
  end
end

# Clean up
task :clean do
  rm_rf 'deployment'
end
