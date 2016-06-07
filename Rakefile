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
task :default => [:version_tracking, :test]
task :release => [:version_tracking, :test]

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

# Version tracking
require 'fileutils'
task :version_tracking do
  FileUtils.rm('.version') if File.exist?('.version')
  File.open('.version', 'w+') {|file| file.write("agent_version: #{getAgentTrackingInfo}")}
  FileUtils.chmod(0444, '.version')
end

def getAgentTrackingInfo
  begin
    commit_id = `git rev-parse HEAD`.chop!
    tracking = "COMMIT_#{commit_id}"
  rescue 
    tracking = "UNKNOWN_VERSION"
  end
end

# Clean up
task :clean do
  rm_rf 'deployment'
end

# Build deb package
desc 'build debian package for installation on xenial'
require 'fpm'
task :package_deb do
  sh "fpm -s dir -t deb -n 'codedeploy-agent' -v 1.0-1.950-bdashrad -x .git --deb-systemd ./init.d/codedeploy-agent.service --after-install ./install_scripts/post_install.sh --before-remove ./install_scripts/pre_remove.sh conf/codedeployagent.yml=/etc/codedeploy-agent/conf/codedeployagent.yml ./=/opt/codedeploy-agent"
end

