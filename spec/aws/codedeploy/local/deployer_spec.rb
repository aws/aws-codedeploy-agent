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
                            "BeforeAllowTraffic"=>["BeforeAllowTraffic"],
                            "AfterAllowTraffic"=>["AfterAllowTraffic"],
                            "ValidateService"=>["ValidateService"]}
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
         "uncompressed"=>false,
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
            OpenStruct.new({
              :format => "TEXT/JSON",
              :payload => {
                "ApplicationId" =>  SAMPLE_FILE_BASENAME.gsub('.','-'),
                "ApplicationName" => SAMPLE_FILE_BASENAME.gsub('.','-'),
                "DeploymentGroupId" => SAMPLE_FILE_BASENAME.gsub('.','-'),
                "DeploymentGroupName" => "LocalFleet",
                "DeploymentId" => TEST_DEPLOYMENT_ID,
                "Revision" => { "RevisionType" => "Local File", "LocalRevision" => {"Location" => SAMPLE_FILE_BUNDLE, "BundleType" => 'tgz'}},
                "AllPossibleLifecycleEvents" => AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS
              }.to_json.to_s
            })).once.ordered
        end
        deployer = AWS::CodeDeploy::Local::Deployer.new
        deployer.execute_events(args)
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
         "uncompressed"=>true,
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
            OpenStruct.new({
              :format => "TEXT/JSON",
              :payload => {
                "ApplicationId" =>  SAMPLE_DIRECTORY_BASENAME.gsub('.','-'),
                "ApplicationName" => SAMPLE_DIRECTORY_BASENAME.gsub('.','-'),
                "DeploymentGroupId" => SAMPLE_DIRECTORY_BASENAME.gsub('.','-'),
                "DeploymentGroupName" => "LocalFleet",
                "DeploymentId" => TEST_DEPLOYMENT_ID,
                "Revision" => { "RevisionType" => "Local Directory", "LocalRevision" => {"Location" => SAMPLE_DIRECTORY_BUNDLE, "BundleType" => 'uncompressed'}},
                "AllPossibleLifecycleEvents" => AWS::CodeDeploy::Local::Deployer::DEFAULT_ORDERED_LIFECYCLE_EVENTS
              }.to_json.to_s
            })).once.ordered
        end
        deployer = AWS::CodeDeploy::Local::Deployer.new
        deployer.execute_events(args)
      end
    end
  end
end
