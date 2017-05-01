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
        CONF_REPO_LOCATION_SUFFIX = '/conf/codedeployagent.yml'

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

        def initialize
          current_directory = Dir.pwd
          InstanceAgent::Log.init(File.join(current_directory, 'codedeploy-local.log'))
          if IS_WINDOWS then self.class.configure_windows_certificate end

          if File.file?(CONF_DEFAULT_LOCATION) && File.readable?(CONF_DEFAULT_LOCATION)
            InstanceAgent::Config.config[:config_file] = CONF_DEFAULT_LOCATION
          else
            InstanceAgent::Config.config[:config_file] = "#{current_directory}#{CONF_REPO_LOCATION_SUFFIX}"
          end

          InstanceAgent::Config.load_config
          InstanceAgent::Platform.util = IS_WINDOWS ? InstanceAgent::WindowsUtil : InstanceAgent::LinuxUtil

          if IS_WINDOWS then InstanceAgent::Config.config[:on_premises_config_file] = "#{WINDOWS_DEFAULT_DIRECTORY}/conf.onpremises.yml" end
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

          all_possible_lifecycle_events = add_download_bundle_and_install_events(ordered_lifecycle_events(args['<event>']))
          spec = build_spec(args['<location>'], bundle_type(args), args['<deployment-group-id>'], all_possible_lifecycle_events)

          command_executor = InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor.new(:hook_mapping => hook_mapping(args['<event>']))
          all_possible_lifecycle_events.each do |name|
            command_executor.execute_command(OpenStruct.new(:command_name => name), spec.clone)
          end
        end

        def ordered_lifecycle_events(events)
          if (events.empty?)
            DEFAULT_ORDERED_LIFECYCLE_EVENTS
          else
            events
          end
        end

        private
        def add_download_bundle_and_install_events(events)
          REQUIRED_LIFECYCLE_EVENTS.select{|hook| !events.include?(hook)} + events
        end

        def hook_mapping(events)
          Hash[ordered_lifecycle_events(events)
            .select{|hook| !REQUIRED_LIFECYCLE_EVENTS.include? hook}
            .map{|h|[h,[h]]}]
        end

        def build_spec(location, bundle_type, deployment_group_id, all_possible_lifecycle_events)
          raise AWS::CodeDeploy::Local::CLIValidator::ValidationError.new("Unknown bundle type #{bundle_type} of #{location}") unless %w(tar zip tgz directory).include? bundle_type

          deployment_id = self.class.random_deployment_id
          puts "Starting to execute deployment from within folder #{InstanceAgent::Config.config[:root_dir]}/#{deployment_group_id}/#{deployment_id}"
          OpenStruct.new({
            :format => "TEXT/JSON",
            :payload => {
              "ApplicationId" => location,
              "ApplicationName" => location,
              "DeploymentGroupId" => deployment_group_id,
              "DeploymentGroupName" => "LocalFleet",
              "DeploymentId" => deployment_id,
              "Revision" => revision(location, bundle_type),
              "AllPossibleLifecycleEvents" => all_possible_lifecycle_events
            }.to_json.to_s
          })
        end

        def self.random_deployment_id
          "d-#{random_alphanumeric(9)}-local"
        end

        def self.random_alphanumeric(length)
          Array.new(length){[*"A".."Z", *"0".."9"].sample}.join
        end

        def bundle_type(args)
          args.select{|k,v| ['tar','tgz','zip','directory'].include?(k) && v}.keys.first
        end

        def revision(location, bundle_type)
          uri = URI.parse(location)
          if (uri.scheme == 's3')
            s3_revision(location, uri, bundle_type)
          elsif (uri.scheme == 'https' && uri.host.end_with?('github.com'))
            github_revision(location, uri)
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

        def github_revision(location, uri)
          if match = uri.path.match(/\/repos\/([^\/]*)\/([^\/]*)\/.*\/(.*)$/i)
            owner, repository_name, commit = match.captures
          else
            raise AWS::CodeDeploy::Local::CLIValidator::ValidationError.new("github location #{location} not in the expected format of 'https://api.github.com/repos/owner/repository_name/tarorzipball/commit'")
          end
          { 'RevisionType' => 'GitHub', 'GitHubRevision' => 
            {'Account' => owner, 
             'Repository' => repository_name, 
             'CommitId' => commit}}
        end

        def local_revision(location, bundle_type)
          if bundle_type == 'directory'
            revision_type = 'Local Directory'
          else
            revision_type = 'Local File'
          end
          { 'RevisionType' => revision_type, 'LocalRevision' => 
            {'Location' => location, 
             'BundleType' => bundle_type}}
        end
      end
    end
  end
end
