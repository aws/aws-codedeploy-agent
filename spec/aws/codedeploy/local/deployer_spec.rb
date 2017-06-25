require 'spec_helper'

require 'aws/codedeploy/local/deployer'
require 'instance_agent/plugins/codedeploy/onpremise_config'

describe AWS::CodeDeploy::Local::Deployer do
  SAMPLE_DIRECTORY_BASENAME = 'sample'
  SAMPLE_FILE_BASENAME = "#{SAMPLE_DIRECTORY_BASENAME}.tgz"
  SAMPLE_FILE_BUNDLE = "#{Dir.pwd}/spec/resource/#{SAMPLE_FILE_BASENAME}"
  SAMPLE_DIRECTORY_BUNDLE = "#{Dir.pwd}/spec/resource/#{SAMPLE_DIRECTORY_BASENAME}"
  GIT_OWNER = 'owner'
  GIT_REPO = 'repo'
  GIT_BRANCH_OR_TAG = 'branchOrTag'
  SAMPLE_GIT_LOCATION_TARBALL = "https://api.github.com/repos/#{GIT_OWNER}/#{GIT_REPO}/tarball/#{GIT_BRANCH_OR_TAG}"
  SAMPLE_GIT_LOCATION_ZIPBALL = "https://api.github.com/repos/#{GIT_OWNER}/#{GIT_REPO}/zipball/#{GIT_BRANCH_OR_TAG}"
  SAMPLE_GIT_LOCATION_DEFAULT_GIT_FORMAT = "https://github.com/#{GIT_OWNER}/#{GIT_REPO}"
  TEST_DEPLOYMENT_ID = 123
  S3_BUCKET = 'bucket'
  S3_KEY = 'key'
  S3_VERSION = 'version'
  S3_ETAG = 'etag'
  SAMPLE_S3_LOCATION = "s3://#{S3_BUCKET}/#{S3_KEY}"
  SAMPLE_S3_LOCATION_WITH_VERSION_AND_ETAG = "s3://#{S3_BUCKET}/#{S3_KEY}?versionId=#{S3_VERSION}&etag=#{S3_ETAG}"
  EXPECTED_HOOK_MAPPING = { "BeforeBlockTraffic"=>["BeforeBlockTraffic"],
                            "AfterBlockTraffic"=>["AfterBlockTraffic"],
                            "ApplicationStop"=>["ApplicationStop"],
                            "BeforeInstall"=>["BeforeInstall"],
                            "AfterInstall"=>["AfterInstall"],
                            "ApplicationStart"=>["ApplicationStart"],
                            "ValidateService"=>["ValidateService"],
                            "BeforeAllowTraffic"=>["BeforeAllowTraffic"],
                            "AfterAllowTraffic"=>["AfterAllowTraffic"]}
  DEPLOYMENT_GROUP_ID = 'deployment-group-id'
  NON_DEFAULT_FILE_EXISTS_BEHAVIOR = 'OVERWRITE'
  let(:test_working_directory) { Dir.mktmpdir }

  before do
    FileUtils.mkdir "#{test_working_directory}/conf"
    @config_file_location = create_config_file(test_working_directory)
    allow(Dir).to receive(:pwd).and_return test_working_directory
    ProcessManager::Config.config[:root_dir] = test_working_directory
    allow(AWS::CodeDeploy::Local::Deployer).to receive(:random_deployment_id).and_return(TEST_DEPLOYMENT_ID)
    allow(File).to receive(:exists?).with(@config_file_location).and_return(true)
    allow(File).to receive(:readable?).with(@config_file_location).and_return(true)
    allow(File).to receive(:exists?).with(AWS::CodeDeploy::Local::Deployer::CONF_DEFAULT_LOCATION).and_return(false)
    allow(File).to receive(:readable?).with(AWS::CodeDeploy::Local::Deployer::CONF_DEFAULT_LOCATION).and_return(false)
    allow(File).to receive(:readable?).with(InstanceAgent::Config.config[:on_premises_config_file]).and_return(false)
  end

  def create_config_file(working_directory, log_dir = nil)
    configuration_contents = <<-CONFIG
---
:log_aws_wire: false
:log_dir: #{log_dir || working_directory}
:pid_dir: #{working_directory}
:program_name: codedeploy-agent
:root_dir: #{working_directory}/deployment-root
:on_premises_config_file: #{InstanceAgent::Config.config[:on_premises_config_file]}
:verbose: true
:wait_between_runs: 1
:proxy_uri:
:max_revisions: 5
    CONFIG

    InstanceAgent::Config.config[:log_dir] = log_dir || working_directory
    InstanceAgent::Config.config[:config_file] = "#{working_directory}/codedeployagent.yml"
    File.open(InstanceAgent::Config.config[:config_file], 'w') { |file| file.write(configuration_contents) }
    InstanceAgent::Config.config[:config_file]
  end

  after do
    FileUtils.rm_rf(test_working_directory)
  end

  describe 'initialize' do
    it 'tries to load configuration' do
      expect(InstanceAgent::Config).to receive(:load_config)
      AWS::CodeDeploy::Local::Deployer.new(@config_file_location)
    end

    it 'tries to load configuration if the configuration file location provided is nil' do
      expect(File).to receive(:file?).with(AWS::CodeDeploy::Local::Deployer::CONF_DEFAULT_LOCATION).and_return(true)
      expect(File).to receive(:readable?).with(AWS::CodeDeploy::Local::Deployer::CONF_DEFAULT_LOCATION).and_return(true)
      expect(File).to receive(:file?).with(InstanceAgent::Config.config[:on_premises_config_file]).and_return(false)
      expect(InstanceAgent::Config).to receive(:load_config)
      AWS::CodeDeploy::Local::Deployer.new(nil)
    end

    it 'tries to load on-premise-configuration from on_premises_config_file if it exists' do
      expect(File).to receive(:file?).with(AWS::CodeDeploy::Local::Deployer::CONF_DEFAULT_LOCATION).and_return(true)
      expect(File).to receive(:readable?).with(AWS::CodeDeploy::Local::Deployer::CONF_DEFAULT_LOCATION).and_return(true)
      expect(File).to receive(:file?).with(InstanceAgent::Config.config[:on_premises_config_file]).and_return(true)
      expect(File).to receive(:readable?).with(InstanceAgent::Config.config[:on_premises_config_file]).and_return(true)
      expect(InstanceAgent::Plugins::CodeDeployPlugin::OnPremisesConfig).to receive(:configure)
      expect(InstanceAgent::Config).to receive(:load_config)
      AWS::CodeDeploy::Local::Deployer.new
    end

    it 'throws ValidationError if configuration file location does not exist' do
      invalid_config_file_location = '/does/not/exist/path'
      expect(File).to receive(:file?).with(invalid_config_file_location).and_return(false)
      expect{AWS::CodeDeploy::Local::Deployer.new(invalid_config_file_location)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "configuration file #{invalid_config_file_location} does not exist or is not readable")
    end

    it 'creates the log directory if it does not exist' do
      new_test_working_directory = "#{test_working_directory}/new_dir"
      FileUtils.mkdir_p new_test_working_directory
      not_yet_existing_directory = "#{new_test_working_directory}/notyetexistsdirectory"
      config_file_location = create_config_file(new_test_working_directory, not_yet_existing_directory)
      expect(FileUtils).to receive(:mkdir_p).with(not_yet_existing_directory).and_call_original
      allow(File).to receive(:readable?).with(config_file_location).and_return(true)
      allow(File).to receive(:exists?).with(config_file_location).and_return(true)
      AWS::CodeDeploy::Local::Deployer.new(config_file_location)
    end
  end

  describe 'execute_events' do
    context 'when local file is specified' do
      let(:args) do
        {"deploy"=>true,
         '--file-exists-behavior'=>InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification::DEFAULT_FILE_EXISTS_BEHAVIOR,
         "--location"=>true,
         "--bundle-location"=>SAMPLE_FILE_BUNDLE,
         "--type"=>'tgz',
         '--deployment-group'=>DEPLOYMENT_GROUP_ID,
         "--help"=>false,
         "--version"=>false}
      end

      it 'deploys the local file and calls the executor to execute all commands' do
        allow(File).to receive(:exists?).with(SAMPLE_FILE_BUNDLE).and_return(true)
        executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

        expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
          with(:hook_mapping => EXPECTED_HOOK_MAPPING).
          and_return(executor)

        AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS.each do |name|
          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => name),
            deployment_spec(SAMPLE_FILE_BUNDLE, 'Local File', 'tgz',
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new(@config_file_location).execute_events(args)
      end

      context 'when script fails with script error' do
        it 'prints the correct error message to the screen and exits' do
          allow(File).to receive(:exists?).with(SAMPLE_FILE_BUNDLE).and_return(true)
          executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

          expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
            with(:hook_mapping => EXPECTED_HOOK_MAPPING).
            and_return(executor)

          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS.first),
            deployment_spec(SAMPLE_FILE_BUNDLE, 'Local File', 'tgz',
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS)).and_raise(
                              InstanceAgent::Plugins::CodeDeployPlugin::ScriptError.new(InstanceAgent::Plugins::CodeDeployPlugin::ScriptError::SCRIPT_FAILED_CODE, 'script-location', nil), 'scripterror')

          deployment_folder = "#{test_working_directory}/deployment-root/deployment-group-id/123"
          expect(STDOUT).to receive(:puts).with("Starting to execute deployment from within folder #{deployment_folder}").once.ordered
          expect(STDOUT).to receive(:puts).with("Your local deployment failed while trying to execute your script at #{deployment_folder}/deployment-archive/script-location").once.ordered
          expect(STDOUT).to receive(:puts).with("See the deployment log at #{deployment_folder}/logs/scripts.log for more details").once.ordered
          expect{AWS::CodeDeploy::Local::Deployer.new(@config_file_location).execute_events(args)}.to raise_error(InstanceAgent::Plugins::CodeDeployPlugin::ScriptError)
        end
      end
    end

    context 'when non-default events specified' do
      NON_DEFAULT_LIFECYCLE_EVENTS = ['Stop','Start','HealthCheck']
      NON_DEFAULT_LIFECYCLE_EVENTS_AFTER_DOWNLOAD_BUNDLE_AND_INSTALL = ['DownloadBundle', 'Install', 'Stop','Start','HealthCheck']

      let(:args) do
        {"deploy"=>true,
         '--file-exists-behavior'=>InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification::DEFAULT_FILE_EXISTS_BEHAVIOR,
         "--location"=>true,
         "--bundle-location"=>SAMPLE_FILE_BUNDLE,
         "--type"=>'tar',
         "--events"=>NON_DEFAULT_LIFECYCLE_EVENTS.join(','),
         '--deployment-group'=>DEPLOYMENT_GROUP_ID,
         "--help"=>false,
         "--version"=>false}
      end

      it 'deploys the local file and calls the executor to execute all specified commands after DownloadBundle and Install commands' do
        allow(File).to receive(:exists?).with(SAMPLE_FILE_BUNDLE).and_return(true)
        executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

        all_possible_lifecycle_events = AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS.to_set.merge(NON_DEFAULT_LIFECYCLE_EVENTS).to_a
        all_expected_hooks = all_possible_lifecycle_events - AWS::CodeDeploy::Local::Deployer::REQUIRED_LIFECYCLE_EVENTS
        expected_hook_mapping = Hash[all_expected_hooks.map{|h|[h,[h]]}]
        expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
          with(:hook_mapping => expected_hook_mapping).
          and_return(executor)

        NON_DEFAULT_LIFECYCLE_EVENTS_AFTER_DOWNLOAD_BUNDLE_AND_INSTALL.each do |name|
          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => name),
            deployment_spec(SAMPLE_FILE_BUNDLE, 'Local File', 'tar',
                            all_possible_lifecycle_events)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new(@config_file_location).execute_events(args)
      end
    end

    context 'when local directory is specified' do
      let(:args) do
        {"deploy"=>true,
         '--file-exists-behavior'=>InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification::DEFAULT_FILE_EXISTS_BEHAVIOR,
         "--location"=>true,
         "--bundle-location"=>SAMPLE_DIRECTORY_BUNDLE,
         "--type"=>'directory',
         '--deployment-group'=>DEPLOYMENT_GROUP_ID,
         "--help"=>false,
         "--version"=>false}
      end

      it 'deploys the local directory and calls the executor to execute all commands' do
        allow(File).to receive(:exists?).with(SAMPLE_DIRECTORY_BUNDLE).and_return(true)
        executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

        expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
          with(:hook_mapping => EXPECTED_HOOK_MAPPING).
          and_return(executor)

        AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS.each do |name|
          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => name),
            deployment_spec(SAMPLE_DIRECTORY_BUNDLE, 'Local Directory', 'directory',
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new(@config_file_location).execute_events(args)
      end
    end

    context 'when file-exists-behavior is specified' do
      let(:args) do
        {'--bundle-location'=>SAMPLE_DIRECTORY_BUNDLE,
         '--type'=>'directory',
         '--deployment-group'=>DEPLOYMENT_GROUP_ID,
         '--file-exists-behavior'=>NON_DEFAULT_FILE_EXISTS_BEHAVIOR}
      end

      it 'deploys the local directory and calls the executor to execute all commands' do
        allow(File).to receive(:exists?).with(SAMPLE_DIRECTORY_BUNDLE).and_return(true)
        executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

        expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
          with(:hook_mapping => EXPECTED_HOOK_MAPPING).
          and_return(executor)

        AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS.each do |name|
          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => name),
            deployment_spec(SAMPLE_DIRECTORY_BUNDLE, 'Local Directory', 'directory',
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS,
                            false, false, DEPLOYMENT_GROUP_ID, NON_DEFAULT_FILE_EXISTS_BEHAVIOR)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new(@config_file_location).execute_events(args)
      end
    end

    context 'when anonymous github tarball endpoint is specified' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         '--file-exists-behavior'=>InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification::DEFAULT_FILE_EXISTS_BEHAVIOR,
         "--bundle-location"=>SAMPLE_GIT_LOCATION_TARBALL,
         "--type"=>'tar',
         '--deployment-group'=>DEPLOYMENT_GROUP_ID,
         "--help"=>false,
         "--version"=>false}
      end

      it 'extracts the endpoint parameters, deploys the downloaded file, and calls the executor to execute all commands' do
        executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

        expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
          with(:hook_mapping => EXPECTED_HOOK_MAPPING).
          and_return(executor)

        AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS.each do |name|
          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => name),
            deployment_spec(SAMPLE_GIT_LOCATION_TARBALL, 'GitHub', 'tar',
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new(@config_file_location).execute_events(args)
      end
    end

    context 'when anonymous github zipball endpoint is specified' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         '--file-exists-behavior'=>InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification::DEFAULT_FILE_EXISTS_BEHAVIOR,
         "--bundle-location"=>SAMPLE_GIT_LOCATION_ZIPBALL,
         "--type"=>'zip',
         '--deployment-group'=>DEPLOYMENT_GROUP_ID,
         "--help"=>false,
         "--version"=>false}
      end

      it 'extracts the endpoint parameters, deploys the downloaded file, and calls the executor to execute all commands' do
        executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

        expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
          with(:hook_mapping => EXPECTED_HOOK_MAPPING).
          and_return(executor)

        AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS.each do |name|
          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => name),
            deployment_spec(SAMPLE_GIT_LOCATION_ZIPBALL, 'GitHub', 'zip',
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new(@config_file_location).execute_events(args)
      end
    end

    context 'when anonymous github endpoint is specified with default github url format' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         '--file-exists-behavior'=>InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification::DEFAULT_FILE_EXISTS_BEHAVIOR,
         "--bundle-location"=>SAMPLE_GIT_LOCATION_DEFAULT_GIT_FORMAT,
         "--type"=>'zip',
         '--deployment-group'=>DEPLOYMENT_GROUP_ID,
         "--help"=>false,
         "--version"=>false}
      end

      it 'extracts the endpoint parameters, deploys the downloaded file, and calls the executor to execute all commands' do
        executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

        expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
          with(:hook_mapping => EXPECTED_HOOK_MAPPING).
          and_return(executor)

        AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS.each do |name|
          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => name),
            deployment_spec(SAMPLE_GIT_LOCATION_DEFAULT_GIT_FORMAT, 'GitHub', 'zip',
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new(@config_file_location).execute_events(args)
      end
    end

    context 'when s3 endpoint is specified' do
      let(:args) do
        {"deploy"=>true,
         '--file-exists-behavior'=>InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification::DEFAULT_FILE_EXISTS_BEHAVIOR,
         "--location"=>true,
         "--bundle-location"=>SAMPLE_S3_LOCATION,
         "--type"=>'zip',
         '--deployment-group'=>DEPLOYMENT_GROUP_ID,
         "--help"=>false,
         "--version"=>false}
      end

      it 'extracts the endpoint parameters, deploys the downloaded file, and calls the executor to execute all commands' do
        executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

        expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
          with(:hook_mapping => EXPECTED_HOOK_MAPPING).
          and_return(executor)

        AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS.each do |name|
          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => name),
            deployment_spec(SAMPLE_S3_LOCATION, 'S3', 'zip',
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new(@config_file_location).execute_events(args)
      end
    end

    context 'when s3 endpoint with version and etag is specified' do
      let(:args) do
        {"deploy"=>true,
         '--file-exists-behavior'=>InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification::DEFAULT_FILE_EXISTS_BEHAVIOR,
         "--location"=>true,
         "--bundle-location"=>SAMPLE_S3_LOCATION_WITH_VERSION_AND_ETAG,
         "--type"=>'zip',
         '--deployment-group'=>DEPLOYMENT_GROUP_ID,
         "--help"=>false,
         "--version"=>false}
      end

      it 'extracts the endpoint parameters, deploys the downloaded file, and calls the executor to execute all commands' do
        executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

        expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
          with(:hook_mapping => EXPECTED_HOOK_MAPPING).
          and_return(executor)

        AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS.each do |name|
          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => name),
            deployment_spec(SAMPLE_S3_LOCATION_WITH_VERSION_AND_ETAG, 'S3', 'zip',
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS,
                            true, true)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new(@config_file_location).execute_events(args)
      end
    end

    def deployment_spec(location, revision_type, bundle_type, all_possible_lifecycle_events, s3revision_includes_version=false, s3revision_includes_etag=false, deployment_group_id=DEPLOYMENT_GROUP_ID, file_exists_behavior=InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification::DEFAULT_FILE_EXISTS_BEHAVIOR)
      revision_data_key = revision_data(revision_type, location, bundle_type, s3revision_includes_version, s3revision_includes_etag).keys.first
      revision_data_value = revision_data(revision_type, location, bundle_type, s3revision_includes_version, s3revision_includes_etag).values.first
      OpenStruct.new({
        :format => "TEXT/JSON",
        :payload => {
          "ApplicationId" =>  location,
          "ApplicationName" => location,
          "DeploymentGroupId" => deployment_group_id,
          "DeploymentGroupName" => "LocalFleet",
          "DeploymentId" => TEST_DEPLOYMENT_ID,
          "AgentActionOverrides" => {"AgentOverrides" => {"FileExistsBehavior" => file_exists_behavior}},
          "Revision" => {"RevisionType" => revision_type,
                         revision_data_key => revision_data_value},
          "AllPossibleLifecycleEvents" => all_possible_lifecycle_events

        }.to_json.to_s
      })
    end

    def revision_data(revision_type, location, bundle_type, s3revision_includes_version, s3revision_includes_etag)
      case revision_type
      when 'S3'
        s3_revision = {'S3Revision' => {
          'Bucket' => S3_BUCKET,
          'Key' => S3_KEY,
          'BundleType' => bundle_type}}

        if s3revision_includes_version
          s3_revision['S3Revision']['Version'] = S3_VERSION
        end

        if s3revision_includes_etag
          s3_revision['S3Revision']['ETag'] = S3_ETAG
        end

        s3_revision
      when 'GitHub'
        {'GitHubRevision' => {
          'Account' => GIT_OWNER,
          'Repository' => GIT_REPO,
          'CommitId' => location.include?(GIT_BRANCH_OR_TAG) ? GIT_BRANCH_OR_TAG : 'HEAD'}}
      when 'Local File', 'Local Directory'
        {'LocalRevision' => {
          'Location' => location,
          'BundleType' => bundle_type}}
      end
    end
  end
end
