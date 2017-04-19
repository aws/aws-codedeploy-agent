require 'spec_helper'

require 'aws/codedeploy/local/deployer'

describe AWS::CodeDeploy::Local::Deployer do
  SAMPLE_DIRECTORY_BASENAME = 'sample'
  SAMPLE_FILE_BASENAME = "#{SAMPLE_DIRECTORY_BASENAME}.tgz"
  SAMPLE_FILE_BUNDLE = "#{Dir.pwd}/spec/resource/#{SAMPLE_FILE_BASENAME}"
  SAMPLE_DIRECTORY_BUNDLE = "#{Dir.pwd}/spec/resource/#{SAMPLE_DIRECTORY_BASENAME}"
  TEST_DEPLOYMENT_ID = 123
  EXPECTED_HOOK_MAPPING = { "BeforeBlockTraffic"=>["BeforeBlockTraffic"],
                            "AfterBlockTraffic"=>["AfterBlockTraffic"],
                            "ApplicationStop"=>["ApplicationStop"],
                            "BeforeInstall"=>["BeforeInstall"],
                            "AfterInstall"=>["AfterInstall"],
                            "ApplicationStart"=>["ApplicationStart"],
                            "ValidateService"=>["ValidateService"],
                            "BeforeAllowTraffic"=>["BeforeAllowTraffic"],
                            "AfterAllowTraffic"=>["AfterAllowTraffic"]}
  let(:test_working_directory) { Dir.mktmpdir }

  before do
    FileUtils.mkdir "#{test_working_directory}/conf"
    config_file_location = "#{test_working_directory}#{AWS::CodeDeploy::Local::Deployer::CONF_REPO_LOCATION_SUFFIX}"
    FileUtils.cp("#{Dir.pwd}#{AWS::CodeDeploy::Local::Deployer::CONF_REPO_LOCATION_SUFFIX}", config_file_location)
    allow(Dir).to receive(:pwd).and_return test_working_directory
    ProcessManager::Config.config[:root_dir] = test_working_directory
    allow(AWS::CodeDeploy::Local::Deployer).to receive(:random_deployment_id).and_return(TEST_DEPLOYMENT_ID)
    allow(File).to receive(:exists?).with(config_file_location).and_return(true)
  end

  after do
    FileUtils.remove_entry(test_working_directory)
  end

  describe 'initialize' do
    it 'tries to load configuration' do
      expect(InstanceAgent::Config).to receive(:load_config)
      AWS::CodeDeploy::Local::Deployer.new
    end

    it 'constructs the correct command executor' do
      expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
        with(:hook_mapping => EXPECTED_HOOK_MAPPING)

      AWS::CodeDeploy::Local::Deployer.new
    end
  end

  describe 'execute_events' do
    context 'when local file is specified' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>SAMPLE_FILE_BUNDLE,
         "--type"=>true,
         "tgz"=>true,
         "zip"=>false,
         "directory"=>false,
         "--event"=>0,
         "<event>"=>[],
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
            deployment_spec(SAMPLE_FILE_BUNDLE, 'Local File',
                            'tgz', SAMPLE_FILE_BASENAME.gsub('.','-'),
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new.execute_events(args)
      end
    end

    context 'when non-default events specified' do
      NON_DEFAULT_LIFECYCLE_EVENTS = ['Stop','Start','HealthCheck']
      NON_DEFAULT_LIFECYCLE_EVENTS_AFTER_DOWNLOAD_BUNDLE_AND_INSTALL = ['DownloadBundle', 'Install', 'Stop','Start','HealthCheck']

      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>SAMPLE_FILE_BUNDLE,
         "--type"=>true,
         "tgz"=>false,
         "tar"=>true,
         "zip"=>false,
         "directory"=>false,
         "--event"=>0,
         "<event>"=>NON_DEFAULT_LIFECYCLE_EVENTS,
         "--help"=>false,
         "--version"=>false}
      end

      it 'deploys the local file and calls the executor to execute all specified commands after DownloadBundle and Install commands' do
        allow(File).to receive(:exists?).with(SAMPLE_FILE_BUNDLE).and_return(true)
        executor = double(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor)

        expect(InstanceAgent::Plugins::CodeDeployPlugin::CommandExecutor).to receive(:new).
          with(:hook_mapping => Hash[NON_DEFAULT_LIFECYCLE_EVENTS.map{|h|[h,[h]]}]).
          and_return(executor)

        NON_DEFAULT_LIFECYCLE_EVENTS_AFTER_DOWNLOAD_BUNDLE_AND_INSTALL.each do |name|
          expect(executor).to receive(:execute_command).with(
            OpenStruct.new(:command_name => name),
            deployment_spec(SAMPLE_FILE_BUNDLE, 'Local File',
                            'tar', SAMPLE_FILE_BASENAME.gsub('.','-'),
                            NON_DEFAULT_LIFECYCLE_EVENTS_AFTER_DOWNLOAD_BUNDLE_AND_INSTALL)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new(args['<event>']).execute_events(args)
      end
    end

    context 'when local directory is specified' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>SAMPLE_DIRECTORY_BUNDLE,
         "--type"=>true,
         "tgz"=>false,
         "zip"=>false,
         "directory"=>true,
         "--event"=>0,
         "<event>"=>[],
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
            deployment_spec(SAMPLE_DIRECTORY_BUNDLE,  'Local Directory',
                            'directory', SAMPLE_DIRECTORY_BASENAME.gsub('.','-'),
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new.execute_events(args)
      end
    end

    context 'when github https endpoint is specified' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>SAMPLE_FILE_BUNDLE,
         "--type"=>true,
         "tgz"=>true,
         "zip"=>false,
         "directory"=>false,
         "--event"=>0,
         "<event>"=>[],
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
            deployment_spec(SAMPLE_FILE_BUNDLE, 'Local File',
                            'tgz', SAMPLE_FILE_BASENAME.gsub('.','-'),
                            AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new.execute_events(args)
      end
    end

    def deployment_spec(location, revision_type, bundle_type, deployment_directory, all_possible_lifecycle_events)
      OpenStruct.new({
        :format => "TEXT/JSON",
        :payload => {
          "ApplicationId" =>  deployment_directory,
          "ApplicationName" => deployment_directory,
          "DeploymentGroupId" => deployment_directory,
          "DeploymentGroupName" => "LocalFleet",
          "DeploymentId" => TEST_DEPLOYMENT_ID,
          "Revision" => { "RevisionType" => revision_type, "LocalRevision" => {"Location" => location, "BundleType" => bundle_type}},
          "AllPossibleLifecycleEvents" => all_possible_lifecycle_events
        }.to_json.to_s
      })
    end
  end
end
