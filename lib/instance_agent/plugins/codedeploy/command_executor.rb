require 'openssl'
require 'fileutils'
require 'aws-sdk-core'
require 'aws-sdk-s3'
require 'zlib'
require 'zip'
require 'instance_metadata'
require 'open-uri'
require 'uri'
require 'set'

require 'instance_agent/plugins/codedeploy/command_poller'
require 'instance_agent/plugins/codedeploy/deployment_specification'
require 'instance_agent/plugins/codedeploy/hook_executor'
require 'instance_agent/plugins/codedeploy/installer'
require 'instance_agent/string_utils'

module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      ARCHIVES_TO_RETAIN = 5
      class CommandExecutor
        class << self
          attr_reader :command_methods
        end

        attr_reader :deployment_system

        InvalidCommandNameFailure = Class.new(Exception)

        def initialize(options = {})
          @deployment_system = "CodeDeploy"
          @hook_mapping = options[:hook_mapping]
          if(!@hook_mapping.nil?)
            map
          end
          begin
            max_revisions = ProcessManager::Config.config[:max_revisions]
            @archives_to_retain = max_revisions.nil?? ARCHIVES_TO_RETAIN : Integer(max_revisions)
            if @archives_to_retain < 1
              raise ArgumentError
            end
          rescue ArgumentError
            log(:error, "Invalid configuration :max_revision=#{max_revisions}")
            Platform.util.quit()
          end
          log(:info, "Archives to retain is: #{@archives_to_retain}}")
        end

        def self.command(name, &blk)
          @command_methods ||= Hash.new
          raise "Received command is not in PascalCase form: #{name.to_s}" unless StringUtils.is_pascal_case(name.to_s)
          method = StringUtils.underscore(name.to_s)
          @command_methods[name] = method

          define_method(method, &blk)
        end

        def is_command_noop?(command_name, deployment_spec)
          deployment_spec = InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(deployment_spec)

          # DownloadBundle and Install are never noops.
          return false if command_name == "Install" || command_name == "DownloadBundle"
          return true if @hook_mapping[command_name].nil?

          @hook_mapping[command_name].each do |lifecycle_event|
            # Although we're not executing any commands here, the HookExecutor handles
            # selecting the correct version of the appspec (last successful or current deployment) for us.
            hook_executor = create_hook_executor(lifecycle_event, deployment_spec)

            is_noop = hook_executor.is_noop?
            if is_noop
              log(:info, "Lifecycle event #{lifecycle_event} is a noop")
            end
            return false unless is_noop
          end

          log(:info, "Noop check completed for command #{command_name}, all lifecycle events are noops.")
          return true
        end

        def total_timeout_for_all_lifecycle_events(command_name, deployment_spec)
          parsed_spec = InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(deployment_spec)
          timeout_sums = ((@hook_mapping || {command_name => []})[command_name] || []).map do |lifecycle_event|
            create_hook_executor(lifecycle_event, parsed_spec).total_timeout_for_all_scripts
          end

          total_timeout = nil
          if timeout_sums.empty?
            log(:info, "Command #{command_name} has no script timeouts specified in appspec.")
          # If any lifecycle events' scripts don't specify a timeout, don't set a value.
          # The default will be the maximum at the server.
          elsif timeout_sums.include?(nil)
            log(:info, "Command #{command_name} has at least one script that does not specify a timeout. " +
              "No timeout override will be sent.")
          else
            total_timeout = timeout_sums.reduce(0) {|running_sum, item| running_sum + item}
            log(:info, "Command #{command_name} has total script timeout #{total_timeout} in appspec.")
          end

          total_timeout
        end

        def execute_command(command, deployment_specification)
          method_name = command_method(command.command_name)
          log(:debug, "Command #{command.command_name} maps to method #{method_name}")

          deployment_specification = InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(deployment_specification)
          log(:debug, "Successfully parsed the deployment spec")

          log(:debug, "Creating deployment root directory #{deployment_root_dir(deployment_specification)}")
          FileUtils.mkdir_p(deployment_root_dir(deployment_specification))
          raise "Error creating deployment root directory #{deployment_root_dir(deployment_specification)}" if !File.directory?(deployment_root_dir(deployment_specification))

          send(method_name, command, deployment_specification)
        end

        def command_method(command_name)
          raise InvalidCommandNameFailure.new("Unsupported command type: #{command_name}.") unless self.class.command_methods.has_key?(command_name)
          self.class.command_methods[command_name]
        end

        command "DownloadBundle" do |cmd, deployment_spec|
          cleanup_old_archives(deployment_spec)
          log(:debug, "Executing DownloadBundle command for execution #{cmd.deployment_execution_id}")

          case deployment_spec.revision_source
          when 'S3'
            download_from_s3(
            deployment_spec,
            deployment_spec.bucket,
            deployment_spec.key,
            deployment_spec.version,
            deployment_spec.etag)
          when 'GitHub'
            download_from_github(
            deployment_spec,
            deployment_spec.external_account,
            deployment_spec.repository,
            deployment_spec.commit_id,
            deployment_spec.anonymous,
            deployment_spec.external_auth_token)
          when 'Local File'
            handle_local_file(
              deployment_spec,
              deployment_spec.local_location)
          when 'Local Directory'
            handle_local_directory(
              deployment_spec,
              deployment_spec.local_location)
          else
            # This should never happen since this is checked during creation of the deployment_spec object.
            raise "Unknown revision type '#{deployment_spec.revision_source}'"
          end

          if deployment_spec.bundle_type != 'directory'
            FileUtils.rm_rf(File.join(deployment_root_dir(deployment_spec), 'deployment-archive'))
            bundle_file = artifact_bundle(deployment_spec)

            unpack_bundle(cmd, bundle_file, deployment_spec)
          end

          FileUtils.mkdir_p(deployment_instructions_dir)
          log(:debug, "Instructions directory created at #{deployment_instructions_dir}")
          update_most_recent_install(deployment_spec)
          nil
        end

        command "Install" do |cmd, deployment_spec|
          log(:debug, "Executing Install command for execution #{cmd.deployment_execution_id}")

          if !File.directory?(deployment_instructions_dir)
            FileUtils.mkdir_p(deployment_instructions_dir)
            log(:debug, "Instructions directory created at #{deployment_instructions_dir}")
          end

          installer = InstanceAgent::Plugins::CodeDeployPlugin::Installer.new(:deployment_instructions_dir => deployment_instructions_dir,
          :deployment_archive_dir => archive_root_dir(deployment_spec),
          :file_exists_behavior => deployment_spec.file_exists_behavior)

          log(:debug, "Installing revision #{deployment_spec.revision} in "+
          "instance group #{deployment_spec.deployment_group_id}")
          installer.install(deployment_spec.deployment_group_id, default_app_spec(deployment_spec))
          update_last_successful_install(deployment_spec)
          nil
        end

        def map
          @hook_mapping.each_pair do |command, lifecycle_events|
            InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor.command command do |cmd, deployment_spec|
              #run the scripts
              script_log = InstanceAgent::Plugins::CodeDeployPlugin::ScriptLog.new
              lifecycle_events.each do |lifecycle_event|
                hook_command = create_hook_executor(lifecycle_event, deployment_spec)
                script_log.concat_log(hook_command.execute)
              end
              script_log.log
            end
          end
        end

        private
        def deployment_root_dir(deployment_spec)
          File.join(ProcessManager::Config.config[:root_dir], deployment_spec.deployment_group_id, deployment_spec.deployment_id)
        end

        private
        def deployment_instructions_dir()
          File.join(ProcessManager::Config.config[:root_dir], 'deployment-instructions')
        end

        private
        def archive_root_dir(deployment_spec)
          File.join(deployment_root_dir(deployment_spec), 'deployment-archive')
        end

        private
        def last_successful_deployment_dir(deployment_group)
          last_successful_install_file_location = last_successful_install_file_path(deployment_group)
          return unless File.exist? last_successful_install_file_location
          File.open last_successful_install_file_location do |f|
            return f.read.chomp
          end
        end

        private
        def most_recent_deployment_dir(deployment_group)
          most_recent_install_file_location = most_recent_install_file_path(deployment_group)
          return unless File.exist? most_recent_install_file_location
          File.open most_recent_install_file_location do |f|
            return f.read.chomp
          end
        end

        private
        def create_hook_executor(lifecycle_event, deployment_spec)
          HookExecutor.new(:lifecycle_event => lifecycle_event,
            :application_name => deployment_spec.application_name,
            :deployment_id => deployment_spec.deployment_id,
            :deployment_group_name => deployment_spec.deployment_group_name,
            :deployment_group_id => deployment_spec.deployment_group_id,
            :deployment_creator => deployment_spec.deployment_creator,
            :deployment_type => deployment_spec.deployment_type,
            :deployment_root_dir => deployment_root_dir(deployment_spec),
            :last_successful_deployment_dir => last_successful_deployment_dir(deployment_spec.deployment_group_id),
            :most_recent_deployment_dir => most_recent_deployment_dir(deployment_spec.deployment_group_id),
            :app_spec_path => deployment_spec.app_spec_path,
            :revision_envs => get_revision_envs(deployment_spec))
        end

        private
        def get_revision_envs(deployment_spec)
          case deployment_spec.revision_source
          when 'S3'
            return get_s3_envs(deployment_spec)
          when 'GitHub'
            return get_github_envs(deployment_spec)
          when 'Local File', 'Local Directory'
            return {}
          else
            raise "Unknown revision type '#{deployment_spec.revision_source}'"
          end
        end

        private
        def get_github_envs(deployment_spec)
          # TODO(CDAGENT-387): expose the repository name and account, but we'll likely need to go through AppSec before doing so.
          return {
            "BUNDLE_COMMIT" => deployment_spec.commit_id
          }
        end

        private
        def get_s3_envs(deployment_spec)
          return {
            "BUNDLE_BUCKET" => deployment_spec.bucket,
            "BUNDLE_KEY" => deployment_spec.key,
            "BUNDLE_VERSION" => deployment_spec.version,
            "BUNDLE_ETAG" => deployment_spec.etag
          }
        end

        private
        def default_app_spec(deployment_spec)
          app_spec_location = app_spec_real_path(deployment_spec)
          validate_app_spec_hooks(app_spec_location, deployment_spec.all_possible_lifecycle_events)
        end

        private
        def validate_app_spec_hooks(app_spec_location, all_possible_lifecycle_events)
          app_spec = ApplicationSpecification::ApplicationSpecification.parse(File.read(app_spec_location))
          app_spec_filename = File.basename(app_spec_location)
          unless all_possible_lifecycle_events.nil?
            app_spec_hooks_plus_hooks_from_mapping = app_spec.hooks.keys.to_set.merge(@hook_mapping.keys).to_a
            unless app_spec_hooks_plus_hooks_from_mapping.to_set.subset?(all_possible_lifecycle_events.to_set)
              unknown_lifecycle_events = app_spec_hooks_plus_hooks_from_mapping - all_possible_lifecycle_events
              raise ArgumentError.new("#{app_spec_filename} file contains unknown lifecycle events: #{unknown_lifecycle_events}")
            end

            app_spec_hooks_plus_hooks_from_default_mapping = app_spec.hooks.keys.to_set.merge(InstanceAgent::Plugins::CodeDeployPlugin::CommandPoller::DEFAULT_HOOK_MAPPING.keys).to_a
            custom_hooks_not_found_in_appspec = custom_lifecycle_events(all_possible_lifecycle_events) - app_spec_hooks_plus_hooks_from_default_mapping
            unless (custom_hooks_not_found_in_appspec).empty?
              raise ArgumentError.new("You specified a lifecycle event which is not a default one and doesn't exist in your #{app_spec_filename} file: #{custom_hooks_not_found_in_appspec.join(',')}")
            end
          end

          app_spec
        end

        def custom_lifecycle_events(all_possible_lifecycle_events)
          all_possible_lifecycle_events - AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS
        end

        private
        def last_successful_install_file_path(deployment_group)
          File.join(deployment_instructions_dir, "#{deployment_group}_last_successful_install")
        end

        private
        def most_recent_install_file_path(deployment_group)
          File.join(deployment_instructions_dir, "#{deployment_group}_most_recent_install")
        end

        private
        def download_from_s3(deployment_spec, bucket, key, version, etag)
          log(:info, "Downloading artifact bundle from bucket '#{bucket}' and key '#{key}', version '#{version}', etag '#{etag}'")
          options = s3_options()
          options = InstanceAgent::Config.common_client_config(options)
          s3 = Aws::S3::Client.new(options)

          File.open(artifact_bundle(deployment_spec), 'wb') do |file|

          begin
            if !version.nil?
              object = s3.get_object({:bucket => bucket, :key => key, :version_id => version}, :target => file)
            else
              object = s3.get_object({:bucket => bucket, :key => key}, :target => file)
            end
          rescue Seahorse::Client::NetworkingError => e
            if e.message.include? "unable to connect to"
              if InstanceAgent::Config.config[:use_fips_mode]
                raise $!, "#{$!}. Check that Fips exists in #{options[:region]}. Or, try using s3 endpoint override.", $!.backtrace
              else
                raise $!, "#{$!}. Try using s3 endpoint override.", $!.backtrace
              end
            else
              raise
            end
          end

            if(!etag.nil? && !(etag.gsub(/"/,'').eql? object.etag.gsub(/"/,'')))
              msg = "Expected deployment artifact bundle etag #{etag} but was actually #{object.etag}"
              log(:error, msg)
              raise RuntimeError, msg
            end
          end
          log(:info, "Download complete from bucket #{bucket} and key #{key}")
        end

        public
        def s3_options
          options = {}
          options[:ssl_ca_directory] = ENV['AWS_SSL_CA_DIRECTORY']
          options[:signature_version] = 'v4'

          region = ENV['AWS_REGION'] || InstanceMetadata.region
          options[:region] = region

          if !InstanceAgent::Config.config[:s3_endpoint_override].to_s.empty?
            ProcessManager::Log.debug("using s3 override endpoint #{InstanceAgent::Config.config[:s3_endpoint_override]}")
            options[:endpoint] = URI(InstanceAgent::Config.config[:s3_endpoint_override])
          elsif InstanceAgent::Config.config[:use_fips_mode]
            ProcessManager::Log.debug("using fips endpoint")
            # There was a recent change to S3 client to decompose the region and use a FIPS endpoint is "fips-" is appended
            # to the region. However, this is such a recent change that we cannot rely on the latest version of the SDK to be loaded.
            # For now, the endpoint will be set directly if FIPS is active but can switch to the S3 method once we have broader support.
            # options[:region] = "fips-#{region}"
            options[:endpoint] = "https://s3-fips.#{region}.amazonaws.com"
          end
          proxy_uri = nil
          if InstanceAgent::Config.config[:proxy_uri]
            proxy_uri = URI(InstanceAgent::Config.config[:proxy_uri])
          end
          options[:http_proxy] = proxy_uri

          if InstanceAgent::Config.config[:log_aws_wire]
            # wire logs might be huge; customers should be careful about turning them on
            # allow 1GB of old wire logs in 64MB chunks
            options[:logger] = Logger.new(
                File.join(InstanceAgent::Config.config[:log_dir], "#{InstanceAgent::Config.config[:program_name]}.aws_wire.log"),
                16,
                64 * 1024 * 1024)
            options[:http_wire_trace] = true
          end

          options
        end

        private
        def download_from_github(deployment_spec, account, repo, commit, anonymous, token)

          retries = 0
          errors = []

          unless (deployment_spec.bundle_type)
            if InstanceAgent::Platform.util.supported_oses.include? 'windows'
              deployment_spec.bundle_type = 'zip'
            else
              deployment_spec.bundle_type = 'tar'
            end
          end

          if deployment_spec.bundle_type == 'zip'
            format = 'zipball'
          elsif deployment_spec.bundle_type == 'tar'
            format = 'tarball'
          else
            raise ArgumentError.new("GitHub revision specified with bundle_type other than zip or tar [bundle_type=#{deployment_spec.bundle_type}")
          end

          uri = URI.parse("https://api.github.com/repos/#{account}/#{repo}/#{format}/#{commit}")
          options = {:ssl_verify_mode => OpenSSL::SSL::VERIFY_PEER, :redirect => true, :ssl_ca_cert => ENV['AWS_SSL_CA_DIRECTORY']}

          if anonymous
            log(:debug, "Anonymous GitHub repository download requested.")
          else
            log(:debug, "Authenticated GitHub repository download requested.")
            options.update({'Authorization' => "token #{token}"})
          end

          begin
            # stream bundle file to disk
            log(:info, "Requesting URL: '#{uri.to_s}'")
            File.open(artifact_bundle(deployment_spec), 'w+b') do |file|
              uri.open(options) do |github|
                log(:debug, "GitHub response: '#{github.meta.to_s}'")

                while (buffer = github.read(8 * 1024 * 1024))
                  file.write buffer
                end
              end
            end
          rescue OpenURI::HTTPError => e
            log(:error, "Could not download bundle at '#{uri.to_s}'. Server returned code #{e.io.status[0]} '#{e.io.status[1]}'")
            log(:debug, "Server returned error response body #{e.io.string}")
            errors << "#{e.io.status[0]} '#{e.io.status[1]}'"

            if retries < 3
              time_to_sleep = (10 * (3 ** retries)) # 10 sec, 30 sec, 90 sec
              log(:info, "Retrying download in #{time_to_sleep} seconds.")
              sleep(time_to_sleep)
              retries += 1
              retry
            else
              raise "Could not download bundle at '#{uri.to_s}' after #{retries} retries. Server returned codes: #{errors.join("; ")}."
            end
          end
        end

        private
        def handle_local_file(deployment_spec, local_location)
          # Symlink local file to the location where download is expected to go
          bundle_file = artifact_bundle(deployment_spec)
          log(:info, "Handle local file #{bundle_file}")
          begin
            File.symlink local_location, bundle_file
          rescue
            #Symlinking fails on windows, copying recursively instead
            FileUtils.cp_r local_location, bundle_file
          end
        end

        private
        def handle_local_directory(deployment_spec, local_location)
          # Copy local directory to the location where a file would have been extracted
          # We copy instead of symlinking in order to preserve revision history
          log(:info, "Handle local directory #{local_location}")
          FileUtils.cp_r local_location, archive_root_dir(deployment_spec)
        end

        private
        def unpack_bundle(cmd, bundle_file, deployment_spec)
          dst = File.join(deployment_root_dir(deployment_spec), 'deployment-archive')

          if "tar".eql? deployment_spec.bundle_type
            InstanceAgent::Platform.util.extract_tar(bundle_file, dst)
          elsif "tgz".eql? deployment_spec.bundle_type
            InstanceAgent::Platform.util.extract_tgz(bundle_file, dst)
          elsif "zip".eql? deployment_spec.bundle_type
            begin
              InstanceAgent::Platform.util.extract_zip(bundle_file, dst)
            rescue Exception => e
              if e.message == "Error extracting zip archive: 50"
                FileUtils.remove_dir(dst)
                # http://infozip.sourceforge.net/FAQ.html#error-codes
                msg = "The disk is (or was) full during extraction."
                log(:warn, msg)
                raise msg
              end
              log(:warn, "#{e.message}, with default system unzip util. Hence falling back to ruby unzip to mitigate any partially unzipped or skipped zip files.")
              Zip::File.open(bundle_file) do |zipfile|
                zipfile.each do |f|
                  file_dst = File.join(dst, f.name)
                  FileUtils.mkdir_p(File.dirname(file_dst))
                  zipfile.extract(f, file_dst) { true }
                end
              end
            end
          else
            InstanceAgent::Platform.util.extract_tar(bundle_file, dst)
          end

          archive_root_files = Dir.entries(dst)
          archive_root_files.delete_if { |name| name == '.' || name == '..' }

          # If the top level of the archive is a directory that contains an appspec,
          # strip that before giving up
          if ((archive_root_files.size == 1) &&
              File.directory?(File.join(dst, archive_root_files[0])) &&
              Dir.entries(File.join(dst, archive_root_files[0])).grep(/appspec/i).any?)
            log(:info, "Stripping leading directory from archive bundle contents.")
            # Move the unpacked files to a temporary location
            tmp_dst = File.join(deployment_root_dir(deployment_spec), 'deployment-archive-temp')
            FileUtils.rm_rf(tmp_dst)
            FileUtils.mv(dst, tmp_dst)

            # Move the top level directory to the intended location
            nested_archive_root = File.join(tmp_dst, archive_root_files[0])
            log(:debug, "Actual archive root at #{nested_archive_root}. Moving to #{dst}")
            FileUtils.mv(nested_archive_root, dst)
            FileUtils.rmdir(tmp_dst)

            log(:debug, Dir.entries(dst).join("; "))
          end
        end

        private
        def update_last_successful_install(deployment_spec)
          File.open(last_successful_install_file_path(deployment_spec.deployment_group_id), 'w+') do |f|
            f.write deployment_root_dir(deployment_spec)
          end
        end

        private
        def update_most_recent_install(deployment_spec)
          File.open(most_recent_install_file_path(deployment_spec.deployment_group_id), 'w+') do |f|
            f.write deployment_root_dir(deployment_spec)
          end
        end

        private
        def cleanup_old_archives(deployment_spec)
          deployment_group = deployment_spec.deployment_group_id
          deployment_archives = Dir.entries(File.join(ProcessManager::Config.config[:root_dir], deployment_group))
          # remove . and ..
          deployment_archives.delete(".")
          deployment_archives.delete("..")

          full_path_deployment_archives = deployment_archives.map{ |f| File.join(ProcessManager::Config.config[:root_dir], deployment_group, f)}
          full_path_deployment_archives.delete(deployment_root_dir(deployment_spec))

          extra = full_path_deployment_archives.size - @archives_to_retain + 1
          return unless extra > 0

          # Never remove the last successful deployment
          last_success = last_successful_deployment_dir(deployment_group)
          full_path_deployment_archives.delete(last_success)

          # Sort oldest -> newest, take first `extra` elements
          oldest_extra = full_path_deployment_archives.sort_by{ |f| File.mtime(f) }.take(extra)

          # Absolute path takes care of relative root directories
          directories = oldest_extra.map{ |f| File.absolute_path(f) }
          log(:debug, "Delete Files #{directories}")
          InstanceAgent::Platform.util.delete_dirs_command(directories)

        end

        private
        def artifact_bundle(deployment_spec)
          File.join(deployment_root_dir(deployment_spec), 'bundle.tar')
        end

        private
        def app_spec_path
          'appspec.yml'
        end

        # Checks for existence the possible extensions of the app_spec_path (.yml and .yaml)
        private
        def app_spec_real_path(deployment_spec)
          app_spec_param_location = File.join(archive_root_dir(deployment_spec), deployment_spec.app_spec_path)
          app_spec_yaml_location = File.join(archive_root_dir(deployment_spec), "appspec.yaml")
          app_spec_yml_location = File.join(archive_root_dir(deployment_spec), "appspec.yml")
          if File.exist? app_spec_param_location
            log(:debug, "Using appspec file #{app_spec_param_location}")
            app_spec_param_location
          elsif File.exist? app_spec_yaml_location
            log(:debug, "Using appspec file #{app_spec_yaml_location}")
            app_spec_yaml_location
          else
            log(:debug, "Using appspec file #{app_spec_yml_location}")
            app_spec_yml_location
          end
        end

        private
        def description
          self.class.to_s
        end

        private
        def log(severity, message)
          raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
          InstanceAgent::Log.send(severity.to_sym, "#{description}: #{message}")
        end
      end
    end
  end
end