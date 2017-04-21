require 'ostruct'
require 'securerandom'

require 'aws/codedeploy/local/cli_validator'
require 'instance_agent/plugins/codedeploy/command_executor'

module AWS
  module CodeDeploy
    module Local
      class Deployer
        CONF_DEFAULT_LOCATION = '/etc/codedeploy-agent/conf/codedeployagent.yml'
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

          if File.file?(CONF_DEFAULT_LOCATION)
            InstanceAgent::Config.config[:config_file] = CONF_DEFAULT_LOCATION
          else
            InstanceAgent::Config.config[:config_file] = "#{current_directory}#{CONF_REPO_LOCATION_SUFFIX}"
          end
          InstanceAgent::Config.load_config
          InstanceAgent::Platform.util = InstanceAgent::LinuxUtil
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

          OpenStruct.new({
            #TODO: Sign JSON instead of passing it around in plaintext so you can avoid supporting special plaintext json messages and always use the signed way
            :format => "TEXT/JSON",
            #TODO: For S3 you need to extract the correct values (bucket, key, tag, etc.) from the location
            :payload => {
              "ApplicationId" => location,
              "ApplicationName" => location,
              "DeploymentGroupId" => deployment_group_id,
              "DeploymentGroupName" => "LocalFleet",
              "DeploymentId" => self.class.random_deployment_id, # needs to be different for each run
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
          if (uri.scheme == 's3' || uri.scheme == 'https' && /s3[a-zA-Z-]*.amazonaws.com/.match(uri.host))
            'S3'
          elsif (uri.scheme == 'https' && uri.host.end_with?('github.com'))
            github_revision(location, uri)
          elsif (uri.scheme == 'file' || uri.scheme.nil?)
            local_revision(location, bundle_type)
          else
            raise AWS::CodeDeploy::Local::CLIValidator::ValidationError.new("unknown location #{location} cannot be determined to be S3, Github, or a local file / directory")
          end
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
