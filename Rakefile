require 'rake'
require 'rake/packagetask'
require 'rake/testtask'
require 'rspec/core/rake_task'
require 'rubygems'
require 'yaml'

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

desc "Run unit tests in spec/"
RSpec::Core::RakeTask.new(:spec)
task :test => :spec

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

# Packaging into a tar
# we need GNU tar to avoid warning when extracting the content on linux systems
def tar
  _tar = `which tar`.chomp
  # we must use GNU tar
  unless `#{_tar} --version`.include?('GNU')
    # probably on a Mac
    _tar = `which gtar`.chomp
    raise 'The GNU tar utility was not found in this system. Please install GNU tar before trying to run this task.' if _tar.empty?
  end
  _tar
end

BIN = "bin"
LIB = "lib"
CERTS = "certs"
CONF = "conf"
VENDOR = "vendor"
VERSION_FILE = ".version"
CONFIG_FILE = "#{CONF}/codedeployagent.yml"
FEATURES = "features"

config = YAML.load(File.read(CONFIG_FILE))

def rubygem_folder
 ruby_version = RUBY_VERSION
 ruby_version_array = ruby_version.split(".")
 ruby_version_array[-1] = "0" # 2.6.x will become 2.6.0
 ruby_version_array.join(".")
end

pkg = "#{Dir.pwd}/pkg" ## Package where the tar will be generated.

desc "Package files into a tar"
task :package do
  # Clean up existing package
  FileUtils.rm_rf(pkg)

  # Set up directories
  bundle_dir = "#{pkg}/#{config[:program_name]}"
  FileUtils.mkdir_p bundle_dir
  FileUtils.mkdir_p "#{bundle_dir}/opt/#{config[:program_name]}/"
  FileUtils.mkdir_p "#{bundle_dir}/opt/#{config[:program_name]}/bin"
  FileUtils.mkdir_p "#{bundle_dir}/etc/#{config[:program_name]}/conf"
  FileUtils.mkdir_p "#{bundle_dir}/etc/init.d/"

  # Copy files
  sh "cp -rf #{BIN} #{bundle_dir}/opt/#{config[:program_name]}/"
  sh "cp -rf #{LIB} #{bundle_dir}/opt/#{config[:program_name]}/"
  sh "cp -f #{CONF}/codedeployagent.yml #{bundle_dir}/etc/#{config[:program_name]}/conf/"
  sh "cp -rf #{CERTS} #{bundle_dir}/opt/#{config[:program_name]}/"
  sh "cp -rf #{VENDOR} #{bundle_dir}/opt/#{config[:program_name]}/"
  sh "cp -rf init.d #{bundle_dir}/etc/"
  sh "cp -f LICENSE #{bundle_dir}/opt/#{config[:program_name]}/"

  sh "sed '/group :test/,$d' Gemfile > #{bundle_dir}/opt/#{config[:program_name]}/Gemfile"
  sh "sed '/add_development_dependency/d' codedeploy_agent.gemspec > #{bundle_dir}/opt/#{config[:program_name]}/codedeploy_agent.gemspec"

  # Build tar
  sh "cd #{bundle_dir} && COPYFILE_DISABLE=true #{tar} --owner=0 --group=0 -cf #{pkg}/#{config[:program_name]}.tar *"
  FileUtils.rm_rf("#{bundle_dir}")
end

# Clean up
task :clean do
  rm_rf 'deployment'
  rm_rf 'pkg'
  rm_rf 'vendor-thirdparty'
end
