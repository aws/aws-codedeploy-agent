require 'uri'
require 'instance_agent/plugins/codedeploy/hook_executor'

module AWS
  module CodeDeploy
    module Local
      #There's no schema validation library for docopt in ruby. This class
      #acts as a way to validate the inputted arguments.
      class CLIValidator
        VALID_TYPES = %w(tgz tar zip directory)

        def validate(args)
          location = args['--bundle-location']
          type = args['--type']

          unless VALID_TYPES.include? type
            raise ValidationError.new("type #{type} is not a valid type. Must be one of #{VALID_TYPES.join(',')}")
          end

          begin
            uri = URI.parse(location)
          rescue URI::InvalidURIError
            raise ValidationError.new("location #{location} is not a valid uri")
          end

          if (uri.scheme == 'http')
            raise ValidationError.new("location #{location} cannot be http, only encrypted (https) url endpoints supported")
          end

          if (uri.scheme != 'https' && uri.scheme != 's3' && !File.exist?(location))
              raise ValidationError.new("location #{location} is specified as a file or directory which does not exist")
          end

          if (type == 'directory' && (uri.scheme != 'https' && uri.scheme != 's3' && File.file?(location)))
              raise ValidationError.new("location #{location} is specified with type directory but it is a file")
          end

          if (type != 'directory' && (uri.scheme != 'https' && uri.scheme != 's3' && File.directory?(location)))
              raise ValidationError.new("location #{location} is specified as a compressed local file but it is a directory")
          end

          if (type == 'directory' && (uri.scheme != 'https' && uri.scheme != 's3' && File.directory?(location)))
            appspec_filename = args['--appspec-filename']
            if !appspec_filename.nil? && !File.exist?("#{location}/#{appspec_filename}")
              raise ValidationError.new("Expecting appspec file at location #{location}/#{appspec_filename} but it is not found there. Please either run the CLI from within a directory containing the #{appspec_filename} file or specify a bundle location containing an #{appspec_filename} file in its root directory")
            end
            if appspec_filename.nil? && !File.exist?("#{location}/appspec.yml") && !File.exist?("#{location}/appspec.yaml")
              raise ValidationError.new("Expecting appspec file at location #{location}/appspec.yml or #{location}/appspec.yaml but it is not found there. Please either run the CLI from within a directory containing the appspec.yml or appspec.yaml file or specify a bundle location containing an appspec.yml or appspec.yaml file in its root directory")
            end
          end

          events = AWS::CodeDeploy::Local::Deployer.events_from_comma_separated_list(args['--events'])
          if events
            if events.include?('DownloadBundle') && any_new_revision_event_or_install_before_download_bundle(events)
              raise ValidationError.new("The only events that can be specified before DownloadBundle are #{events_using_previous_successfuly_deployment_revision.join(',')}. Please fix the order of your specified events: #{args['--events']}")
            end

            if events.include?('Install') && any_new_revision_event_before_install(events)
              raise ValidationError.new("The only events that can be specified before Install are #{events_using_previous_successfuly_deployment_revision.push('DownloadBundle', 'BeforeInstall').join(',')}. Please fix the order of your specified events: #{args['--events']}")
            end
          end

          args
        end

        def any_new_revision_event_or_install_before_download_bundle(events)
          events_using_new_revision.push('Install').any? do |event_not_allowed_before_download_bundle|
            events.take_while{|e| e != 'DownloadBundle'}.include? event_not_allowed_before_download_bundle
          end
        end

        def any_new_revision_event_before_install(events)
          events_using_new_revision.any? do |event_not_allowed_before_install|
            events.take_while{|e| e != 'Install'}.include? event_not_allowed_before_install
          end
        end

        def events_using_previous_successfuly_deployment_revision
          InstanceAgent::Plugins::CodeDeployPlugin::HookExecutor::MAPPING_BETWEEN_HOOKS_AND_DEPLOYMENTS.select do |key,value|
            value == InstanceAgent::Plugins::CodeDeployPlugin::HookExecutor::LAST_SUCCESSFUL_DEPLOYMENT
          end.keys
        end

        def events_using_new_revision
          InstanceAgent::Plugins::CodeDeployPlugin::HookExecutor::MAPPING_BETWEEN_HOOKS_AND_DEPLOYMENTS.select do |key,value|
            value != InstanceAgent::Plugins::CodeDeployPlugin::HookExecutor::LAST_SUCCESSFUL_DEPLOYMENT && key != 'BeforeInstall'
          end.keys
        end

        class ValidationError < StandardError
        end
      end
    end
  end
end
