require 'openssl'
require 'fileutils'
require 'aws-sdk-core'
require 'zlib'
require 'zip'
require 'instance_metadata'
require 'open-uri'
require 'uri'

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
        end

        def self.command(name, &blk)
          @command_methods ||= Hash.new

          method = Seahorse::Util.underscore(name).to_sym
          @command_methods[name] = method

          define_method(method, &blk)
        end

        def execute_command(command, deployment_specification)
          method_name = command_method(command.command_name)
          log(:debug, "Command #{command.command_name} maps to method #{method_name}")

          deployment_specification = DeploymentSpecification.parse(deployment_specification)
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
          cleanup_old_archives(deployment_spec.deployment_group_id)
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
          else
            # This should never happen since this is checked during creation of the deployment_spec object.
            raise "Unknown revision type '#{deployment_spec.revision_source}'"
          end

          FileUtils.rm_rf(File.join(deployment_root_dir(deployment_spec), 'deployment-archive'))
          bundle_file = artifact_bundle(deployment_spec)

          unpack_bundle(cmd, bundle_file, deployment_spec)

          nil
        end

        command "Install" do |cmd, deployment_spec|
          log(:debug, "Executing Install command for execution #{cmd.deployment_execution_id}")

          FileUtils.mkdir_p(deployment_instructions_dir)
          log(:debug, "Instructions directory created at #{deployment_instructions_dir}")

          installer = Installer.new(:deployment_instructions_dir => deployment_instructions_dir,
          :deployment_archive_dir => archive_root_dir(deployment_spec))

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
              script_log = ScriptLog.new
              lifecycle_events.each do |lifecycle_event|
                hook_command = HookExecutor.new(:lifecycle_event => lifecycle_event,
                :application_name => deployment_spec.application_name,
                :deployment_id => deployment_spec.deployment_id,
                :deployment_group_name => deployment_spec.deployment_group_name,
                :deployment_root_dir => deployment_root_dir(deployment_spec),
                :last_successful_deployment_dir => last_successful_deployment_dir(deployment_spec.deployment_group_id),
                :app_spec_path => app_spec_path)
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
          last_install_file_location = last_install_file_path(deployment_group)
          return unless File.exist? last_install_file_location
          File.open last_install_file_location do |f|
            return f.read.chomp
          end
        end

        private
        def default_app_spec(deployment_spec)
          default_app_spec_location = File.join(archive_root_dir(deployment_spec), app_spec_path)
          log(:debug, "Checking for app spec in #{default_app_spec_location}")
          app_spec =  ApplicationSpecification::ApplicationSpecification.parse(File.read(default_app_spec_location))
        end

        private
        def last_install_file_path(deployment_group)
          File.join(deployment_instructions_dir, "#{deployment_group}_last_successful_install")
        end

        private
        def download_from_s3(deployment_spec, bucket, key, version, etag)
          log(:debug, "Downloading artifact bundle from bucket '#{bucket}' and key '#{key}', version '#{version}', etag '#{etag}'")
          region = ENV['AWS_REGION'] || InstanceMetadata.region
          
          proxy_uri = nil
          if InstanceAgent::Config.config[:proxy_uri]
            proxy_uri = URI(InstanceAgent::Config.config[:proxy_uri])
          end

          if InstanceAgent::Config.config[:log_aws_wire]
            s3 = Aws::S3::Client.new(
            :region => region,
            :ssl_ca_directory => ENV['AWS_SSL_CA_DIRECTORY'],
            # wire logs might be huge; customers should be careful about turning them on
            # allow 1GB of old wire logs in 64MB chunks
            :logger => Logger.new(
            File.join(InstanceAgent::Config.config[:log_dir], "#{InstanceAgent::Config.config[:program_name]}.aws_wire.log"),
            16,
            64 * 1024 * 1024),
            :http_wire_trace => true,
            :signature_version => 'v4',
            :http_proxy => proxy_uri)
          else
            s3 = Aws::S3::Client.new(
            :region => region,
            :ssl_ca_directory => ENV['AWS_SSL_CA_DIRECTORY'],
            :signature_version => 'v4',
            :http_proxy => proxy_uri)
          end

          File.open(artifact_bundle(deployment_spec), 'wb') do |file|

            if !version.nil?
              object = s3.get_object({:bucket => bucket, :key => key, :version_id => version}, :target => file)
            else
              object = s3.get_object({:bucket => bucket, :key => key}, :target => file)
            end

            if(!etag.nil? && !(etag.gsub(/"/,'').eql? object.etag.gsub(/"/,'')))
              msg = "Expected deployment artifact bundle etag #{etag} but was actually #{object.etag}"
              log(:error, msg)
              raise RuntimeError, msg
            end
          end
          log(:debug, "Download complete from bucket #{bucket} and key #{key}")
        end

        private
        def download_from_github(deployment_spec, account, repo, commit, anonymous, token)

          retries = 0
          errors = []

          if InstanceAgent::Platform.util.supported_oses.include? 'windows'
            deployment_spec.bundle_type = 'zip'
            format = 'zipball'
          else
            deployment_spec.bundle_type = 'tar'
            format = 'tarball'
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
              log(:debug, "Retrying download in #{time_to_sleep} seconds.")
              sleep(time_to_sleep)
              retries += 1
              retry
            else
              raise "Could not download bundle at '#{uri.to_s}' after #{retries} retries. Server returned codes: #{errors.join("; ")}."
            end
          end
        end

        private
        def unpack_bundle(cmd, bundle_file, deployment_spec)
          strip_leading_directory = deployment_spec.revision_source == 'GitHub'

          if strip_leading_directory
            # Extract to a temporary directory first so we can move the files around
            dst = File.join(deployment_root_dir(deployment_spec), 'deployment-archive-temp')
            actual_dst = File.join(deployment_root_dir(deployment_spec), 'deployment-archive')
            FileUtils.rm_rf(dst)
          else
            dst = File.join(deployment_root_dir(deployment_spec), 'deployment-archive')
          end

          if "tar".eql? deployment_spec.bundle_type
            InstanceAgent::Platform.util.extract_tar(bundle_file, dst)
          elsif "tgz".eql? deployment_spec.bundle_type
            InstanceAgent::Platform.util.extract_tgz(bundle_file, dst)
          elsif "zip".eql? deployment_spec.bundle_type
            Zip::File.open(bundle_file) do |zipfile|
              zipfile.each do |f|
                file_dst = File.join(dst, f.name)
                FileUtils.mkdir_p(File.dirname(file_dst))
                zipfile.extract(f, file_dst)
              end
            end
          else
            # If the bundle was a generated through a Sabini Repository
            # it will be in tar format, and it won't have a bundle type
            InstanceAgent::Platform.util.extract_tar(bundle_file, dst)
          end

          if strip_leading_directory
            log(:info, "Stripping leading directory from archive bundle contents.")

            # Find leading directory to remove
            archive_root_files = Dir.entries(dst)
            archive_root_files.delete_if { |name| name == '.' || name == '..' }

            if (archive_root_files.size != 1)
              log(:warn, "Expected archive to have a single root directory containing the actual bundle root, but it had #{archive_root_files.size} entries instead. Skipping leading directory removal and using archive as is.")
              FileUtils.mv(dst, actual_dst)
              return
            end

            nested_archive_root = File.join(dst, archive_root_files[0])
            log(:debug, "Actual archive root at #{nested_archive_root}. Moving to #{actual_dst}")

            FileUtils.mv(nested_archive_root, actual_dst)
            FileUtils.rmdir(dst)

            log(:debug, Dir.entries(actual_dst).join("; "))
          end
        end

        private
        def update_last_successful_install(deployment_spec)
          File.open(last_install_file_path(deployment_spec.deployment_group_id), 'w+') do |f|
            f.write deployment_root_dir(deployment_spec)
          end
        end

        private
        def cleanup_old_archives(deployment_group)
          deployment_archives = Dir.entries(File.join(ProcessManager::Config.config[:root_dir], deployment_group))
          # remove . and ..
          deployment_archives.delete(".")
          deployment_archives.delete("..")

          full_path_deployment_archives = deployment_archives.map{ |f| File.join(ProcessManager::Config.config[:root_dir], deployment_group, f)}
          
          extra = full_path_deployment_archives.size - ARCHIVES_TO_RETAIN
          return unless extra > 0

          # Never remove the last successful deployment
          last_success = last_successful_deployment_dir(deployment_group)
          full_path_deployment_archives.delete(last_success)

          # Sort oldest -> newest, take first `extra` elements
          oldest_extra = full_path_deployment_archives.sort_by{ |f| File.mtime(f) }.take(extra)

          # Absolute path takes care of relative root directories
          directories = oldest_extra.map{ |f| File.absolute_path(f) }
          FileUtils.rm_rf(directories)

        end

        private
        def artifact_bundle(deployment_spec)
          File.join(deployment_root_dir(deployment_spec), 'bundle.tar')
        end

        private
        def app_spec_path
          'appspec.yml'
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
