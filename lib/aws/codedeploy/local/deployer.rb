require 'ostruct'
require 'securerandom'

require 'aws/codedeploy/local/cli_validator'
require 'instance_agent/plugins/codedeploy/command_poller'
require 'instance_agent/plugins/codedeploy/command_executor'

module AWS
  module CodeDeploy
    module Local
      class Deployer
        CONF_DEFAULT_LOCATION = '/etc/codedeploy-agent/conf/codedeployagent.yml'
        CONF_REPO_LOCATION_SUFFIX = '/conf/codedeployagent.yml'

        DEFAULT_ORDERED_LIFECYCLE_EVENTS = %w(ApplicationStop
                                              DownloadBundle
                                              BeforeInstall
                                              Install
                                              AfterInstall
                                              ApplicationStart
                                              BeforeBlockTraffic
                                              AfterBlockTraffic
                                              BeforeAllowTraffic
                                              AfterAllowTraffic
                                              ValidateService)

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

          @command_executor = InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor.new(:hook_mapping => InstanceAgent::Plugins::CodeDeployPlugin::CommandPoller::DEFAULT_HOOK_MAPPING)
        end

        def execute_events(args)
          args = AWS::CodeDeploy::Local::CLIValidator.new.validate(args)
          all_possible_lifecycle_events = ordered_lifecycle_events(args['<event>'])
          spec = build_spec(args['<location>'], bundle_type(args), all_possible_lifecycle_events)

          all_possible_lifecycle_events.each do |name|
            @command_executor.execute_command(OpenStruct.new(:command_name => name), spec.clone)
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
        def build_spec(location, bundle_type, all_possible_lifecycle_events)
          raise AWS::CodeDeploy::Local::CLIValidator::ValidationError.new("Unknown bundle type #{bundle_type} of #{location}") unless %w(tar zip tgz uncompressed).include? bundle_type

          OpenStruct.new({
            :format => "TEXT/JSON",
            #TODO: For S3 you need to extract the correct values (bucket, key, tag, etc.) from the location
            #TODO: For Github you need to extract out the token from the location
            :payload => {
              "ApplicationId" =>  File.basename(location),
              "ApplicationName" => File.basename(location),
              "DeploymentGroupId" => File.basename(location).gsub('.','-'), # The deployment directory is used so we replace periods with dashes
              "DeploymentGroupName" => "LocalFleet",
              "DeploymentId" => SecureRandom.uuid, # needs to be different for each run
              "Revision" => { "RevisionType" => revision_type(location, bundle_type), "LocalRevision" => {"Location" => location, "BundleType" => bundle_type}},
              "AllPossibleLifecycleEvents" => all_possible_lifecycle_events
            }.to_json.to_s
          })
        end

        def bundle_type(args)
          args.select{|k,v| ['tar','tgz','zip','uncompressed'].include?(k) && v}.keys.first
        end

        def revision_type(location, bundle_type)
          uri = URI.parse(location)
          if (uri.scheme == 's3' || uri.scheme == 'https' && /s3[a-zA-Z-]*.amazonaws.com/.match(uri.host))
            'S3'
          elsif (uri.scheme == 'https' && uri.host.end_with?('github.com'))
            'GitHub'
          elsif (uri.scheme == 'file' || uri.scheme.nil?)
            if bundle_type == 'uncompressed'
              'Local Directory'
            else
              'Local File'
            end
          else
            raise AWS::CodeDeploy::Local::CLIValidator::ValidationError.new("unknown location #{location} cannot be determined to be S3, Github, or a local file / directory")
          end
        end
      end
    end
  end
end
