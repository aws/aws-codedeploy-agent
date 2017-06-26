require 'ostruct'
require 'securerandom'
require 'rbconfig'

require 'aws/codedeploy/local/cli_validator'
require 'instance_agent'
require 'instance_agent/log'
require 'instance_agent/platform/windows_util'
require 'instance_agent/plugins/codedeploy/command_executor'
require 'instance_agent/plugins/codedeploy/onpremise_config'

module AWS
  module CodeDeploy
    module Local
      class Deployer
        IS_WINDOWS = (RbConfig::CONFIG['host_os'] =~ /mswin|mingw|cygwin/)
        WINDOWS_DEFAULT_DIRECTORY = File.join(ENV['PROGRAMDATA'] || '/', 'Amazon/CodeDeploy')
        CONF_DEFAULT_LOCATION = IS_WINDOWS ? "#{WINDOWS_DEFAULT_DIRECTORY}/conf.yml" : '/etc/codedeploy-agent/conf/codedeployagent.yml'
        DEFAULT_DEPLOYMENT_GROUP_ID = 'default-local-deployment-group'

        DEFAULT_ORDERED_LIFECYCLE_EVENTS = %w(BeforeBlockTraffic
                                              AfterBlockTraffic
                                              ApplicationStop
                                              DownloadBundle
                                              BeforeInstall
                                              Install
                                              AfterInstall
                                              ApplicationStart
                                              ValidateService
                                              BeforeAllowTraffic
                                              AfterAllowTraffic)

        REQUIRED_LIFECYCLE_EVENTS = %w(DownloadBundle Install)

        def initialize(configuration_file_location = CONF_DEFAULT_LOCATION)
          configuration_file_location ||= CONF_DEFAULT_LOCATION # Default gets set this way even if the input is nil
          if IS_WINDOWS then self.class.configure_windows_certificate end

          if File.file?(configuration_file_location) && File.readable?(configuration_file_location)
            InstanceAgent::Config.config[:config_file] = configuration_file_location
          else
            raise AWS::CodeDeploy::Local::CLIValidator::ValidationError.new("configuration file #{configuration_file_location} does not exist or is not readable")
          end

          InstanceAgent::Config.load_config

          FileUtils.mkdir_p(InstanceAgent::Config.config[:log_dir])
          InstanceAgent::Log.init(File.join(InstanceAgent::Config.config[:log_dir], 'codedeploy-local.log'))
          InstanceAgent::Platform.util = IS_WINDOWS ? InstanceAgent::WindowsUtil : InstanceAgent::LinuxUtil

          if File.file?(InstanceAgent::Config.config[:on_premises_config_file]) && File.readable?(InstanceAgent::Config.config[:on_premises_config_file])
            InstanceAgent::Plugins::CodeDeployPlugin::OnPremisesConfig.configure
          end
        end

        def self.configure_windows_certificate
          cert_dir = File.expand_path(File.join(File.dirname(__FILE__), '..\..\..\..\certs'))
          Aws.config[:ssl_ca_bundle] = File.join(cert_dir, 'windows-ca-bundle.crt')
          ENV['AWS_SSL_CA_DIRECTORY'] = File.join(cert_dir, 'windows-ca-bundle.crt')
          ENV['SSL_CERT_FILE'] = File.join(cert_dir, 'windows-ca-bundle.crt')
        end

        def execute_events(args)
          args = AWS::CodeDeploy::Local::CLIValidator.new.validate(args)
          # Sets default value of deployment_group_id if it's missing
          deployment_group_id = args['--deployment-group']
          events = events_from_comma_separated_list(args['--events'])

          spec = build_spec(args['--bundle-location'], args['--type'], deployment_group_id, args['--file-exists-behavior'], all_possible_lifecycle_events(events))

          command_executor = InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor.new(:hook_mapping => hook_mapping(events))
          all_lifecycle_events_to_execute = add_download_bundle_and_install_events(ordered_lifecycle_events(events))

          begin
            all_lifecycle_events_to_execute.each do |name|
              command_executor.execute_command(OpenStruct.new(:command_name => name), spec.clone)
            end
          rescue InstanceAgent::Plugins::CodeDeployPlugin::ScriptError => e
            print_script_error_message(e, deployment_group_id, @deployment_id)
            raise
          ensure
            print_deployment_log_location(deployment_group_id, @deployment_id)
          end
        end

        def ordered_lifecycle_events(events)
          if (events.nil? || events.empty?)
            DEFAULT_ORDERED_LIFECYCLE_EVENTS
          else
            events
          end
        end

        private
        def add_download_bundle_and_install_events(events)
          REQUIRED_LIFECYCLE_EVENTS.select{|hook| !events.include?(hook)} + events
        end

        def all_possible_lifecycle_events(events)
          DEFAULT_ORDERED_LIFECYCLE_EVENTS.to_set.merge(ordered_lifecycle_events(events)).to_a
        end

        def hook_mapping(events)
          all_events_plus_default_events_minus_required_events = DEFAULT_ORDERED_LIFECYCLE_EVENTS.to_set.merge(ordered_lifecycle_events(events)) - REQUIRED_LIFECYCLE_EVENTS
          Hash[all_events_plus_default_events_minus_required_events.map{|h|[h,[h]]}]
        end

        def build_spec(location, bundle_type, deployment_group_id, file_exists_behavior, all_possible_lifecycle_events)
          @deployment_id = self.class.random_deployment_id
          puts "Starting to execute deployment from within folder #{deployment_folder(deployment_group_id, @deployment_id)}"
          OpenStruct.new({
            :format => "TEXT/JSON",
            :payload => {
              "ApplicationId" => location,
              "ApplicationName" => location,
              "DeploymentGroupId" => deployment_group_id,
              "DeploymentGroupName" => "LocalFleet",
              "DeploymentId" => @deployment_id,
              "AgentActionOverrides" => {"AgentOverrides" => {"FileExistsBehavior" => file_exists_behavior}},
              "Revision" => revision(location, bundle_type),
              "AllPossibleLifecycleEvents" => all_possible_lifecycle_events
            }.to_json.to_s
          })
        end

        def events_from_comma_separated_list(comma_separated_events)
          if (comma_separated_events.nil?)
            comma_separated_events
          else
            comma_separated_events.split(',')
          end
        end

        def self.random_deployment_id
          "d-#{random_alphanumeric(9)}-local"
        end

        def self.random_alphanumeric(length)
          Array.new(length){[*"A".."Z", *"0".."9"].sample}.join
        end

        def deployment_folder(deployment_group_id, deployment_id)
          "#{InstanceAgent::Config.config[:root_dir]}/#{deployment_group_id}/#{deployment_id}"
        end

        def revision(location, bundle_type)
          uri = URI.parse(location)
          if (uri.scheme == 's3')
            s3_revision(location, uri, bundle_type)
          elsif (uri.scheme == 'https' && uri.host.end_with?('github.com'))
            github_revision(location, uri, bundle_type)
          elsif (uri.scheme == 'file' || uri.scheme.nil? || (uri.scheme.size == 1 && /[[:alpha:]]/.match(uri.scheme.chars.first)))
            #For windows we want to check if the scheme is a single drive letter like C:/Users/username/file.zip
            #unlike linux whose paths are usually scheme-less like with /home/user/file.zip
            local_revision(location, bundle_type)
          else
            raise AWS::CodeDeploy::Local::CLIValidator::ValidationError.new("unknown location #{location} cannot be determined to be S3, Github, or a local file / directory")
          end
        end

        def s3_revision(location, uri, bundle_type)
          bucket = uri.host
          if (uri.path[0] != '/')
            raise AWS::CodeDeploy::Local::CLIValidator::ValidationError.new("s3 location #{location} not in the expected format of 's3://bucket/key'")
          end

          key = uri.path[1..-1]

          s3_revision = { 'RevisionType' => 'S3', 'S3Revision' =>
            {'Bucket' => bucket,
             'Key' => key,
             'BundleType' => bundle_type}}

          unless (uri.query.nil? || uri.query.empty?)
            versionAndETagParameters = Hash[URI::decode_www_form(uri.query)]

            if versionAndETagParameters.has_key?('versionId')
              s3_revision['S3Revision']['Version'] = versionAndETagParameters['versionId']
            end

            if versionAndETagParameters.has_key?('etag')
              s3_revision['S3Revision']['ETag'] = versionAndETagParameters['etag']
            end
          end

          s3_revision
        end

        def github_revision(location, uri, bundle_type)
          if uri.host == 'github.com' && match = uri.path.match(/\/([^\/]*)\/(.*)$/i)
            owner, repository_name = match.captures
            commit = 'HEAD'
          elsif match = uri.path.match(/\/repos\/([^\/]*)\/([^\/]*)\/.*\/(.*)$/i)
            owner, repository_name, commit = match.captures
          else
              raise AWS::CodeDeploy::Local::CLIValidator::ValidationError.new("github location #{location} not in the expected format of 'https://github.com/<owner>/<repository_name>'(assumes HEAD commit) or 'https://api.github.com/repos/<owner>/<repository_name>/tarball/<commit>' or 'https://api.github.com/repos/<owner>/<repository_name>/zipball/<commit>'")
          end
          { 'RevisionType' => 'GitHub', 'GitHubRevision' =>
            {'Account' => owner,
             'Repository' => repository_name,
             'CommitId' => commit,
             'BundleType' => bundle_type == 'zip' || bundle_type == 'tar' ? bundle_type : 'zip'
            }}
        end

        def local_revision(location, bundle_type)
          if bundle_type == 'directory'
            revision_type = 'Local Directory'
          else
            revision_type = 'Local File'
          end
          { 'RevisionType' => revision_type, 'LocalRevision' =>
            {'Location' => File.expand_path(location),
             'BundleType' => bundle_type}}
        end

        def print_script_error_message(script_error, deployment_group_id, deployment_id)
          puts "Your local deployment failed while trying to execute your script at #{deployment_folder(deployment_group_id, deployment_id)}/deployment-archive/#{script_error.script_name}"
        end

        def print_deployment_log_location(deployment_group_id, deployment_id)
          puts "See the deployment log at #{deployment_folder(deployment_group_id, deployment_id)}/#{InstanceAgent::Plugins::CodeDeployPlugin::ScriptLog::SCRIPT_LOG_FILE_RELATIVE_LOCATION} for more details"
        end
      end
    end
  end
end
