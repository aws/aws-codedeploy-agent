require 'spec_helper'

require 'aws/codedeploy/local/deployer'

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
  let(:test_working_directory) { Dir.mktmpdir }

  before do
    FileUtils.mkdir "#{test_working_directory}/conf"
    config_file_location = "#{test_working_directory}#{AWS::CodeDeploy::Local::Deployer::CONF_REPO_LOCATION_SUFFIX}"
    FileUtils.cp("#{Dir.pwd}#{AWS::CodeDeploy::Local::Deployer::CONF_REPO_LOCATION_SUFFIX}", config_file_location)
    allow(Dir).to receive(:pwd).and_return test_working_directory
    ProcessManager::Config.config[:root_dir] = test_working_directory
    allow(AWS::CodeDeploy::Local::Deployer).to receive(:random_deployment_id).and_return(TEST_DEPLOYMENT_ID)
    allow(File).to receive(:exists?).with(config_file_location).and_return(true)
    allow(File).to receive(:exists?).with(AWS::CodeDeploy::Local::Deployer::CONF_DEFAULT_LOCATION).and_return(true)
  end

  after do
    FileUtils.remove_entry(test_working_directory)
  end

  describe 'initialize' do
    it 'tries to load configuration' do
      expect(InstanceAgent::Config).to receive(:load_config)
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
         '<deployment-group-id>'=>DEPLOYMENT_GROUP_ID,
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
         '<deployment-group-id>'=>DEPLOYMENT_GROUP_ID,
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
            deployment_spec(SAMPLE_FILE_BUNDLE, 'Local File', 'tar',
                            NON_DEFAULT_LIFECYCLE_EVENTS_AFTER_DOWNLOAD_BUNDLE_AND_INSTALL)).once.ordered
        end
        AWS::CodeDeploy::Local::Deployer.new.execute_events(args)
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
         '<deployment-group-id>'=>DEPLOYMENT_GROUP_ID,
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
        AWS::CodeDeploy::Local::Deployer.new.execute_events(args)
      end
    end

    context 'when anonymous github tarball endpoint is specified' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>SAMPLE_GIT_LOCATION_TARBALL,
         "--type"=>true,
         "tgz"=>false,
         "tar"=>true,
         "zip"=>false,
         "directory"=>false,
         "--event"=>0,
         "<event>"=>[],
         '<deployment-group-id>'=>DEPLOYMENT_GROUP_ID,
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
        AWS::CodeDeploy::Local::Deployer.new.execute_events(args)
      end
    end

    context 'when anonymous github zipball endpoint is specified' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>SAMPLE_GIT_LOCATION_ZIPBALL,
         "--type"=>true,
         "tgz"=>false,
         "zip"=>true,
         "directory"=>false,
         "--event"=>0,
         "<event>"=>[],
         '<deployment-group-id>'=>DEPLOYMENT_GROUP_ID,
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
        AWS::CodeDeploy::Local::Deployer.new.execute_events(args)
      end
    end

    context 'when s3 endpoint is specified' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>SAMPLE_S3_LOCATION,
         "--type"=>true,
         "tgz"=>false,
         "zip"=>true,
         "directory"=>false,
         "--event"=>0,
         "<event>"=>[],
         '<deployment-group-id>'=>DEPLOYMENT_GROUP_ID,
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
        AWS::CodeDeploy::Local::Deployer.new.execute_events(args)
      end
    end

    context 'when s3 endpoint with version and etag is specified' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>SAMPLE_S3_LOCATION_WITH_VERSION_AND_ETAG,
         "--type"=>true,
         "tgz"=>false,
         "zip"=>true,
         "directory"=>false,
         "--event"=>0,
         "<event>"=>[],
         '<deployment-group-id>'=>DEPLOYMENT_GROUP_ID,
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
        AWS::CodeDeploy::Local::Deployer.new.execute_events(args)
      end
    end

    def deployment_spec(location, revision_type, bundle_type, all_possible_lifecycle_events, s3revision_includes_version=false, s3revision_includes_etag= false)
      revision_data_key = revision_data(revision_type, location, bundle_type, s3revision_includes_version, s3revision_includes_etag).keys.first
      revision_data_value = revision_data(revision_type, location, bundle_type, s3revision_includes_version, s3revision_includes_etag).values.first
      OpenStruct.new({
        :format => "TEXT/JSON",
        :payload => {
          "ApplicationId" =>  location,
          "ApplicationName" => location,
          "DeploymentGroupId" => DEPLOYMENT_GROUP_ID,
          "DeploymentGroupName" => "LocalFleet",
          "DeploymentId" => TEST_DEPLOYMENT_ID,
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
          'CommitId' => GIT_BRANCH_OR_TAG}}
      when 'Local File', 'Local Directory'
        {'LocalRevision' => {
          'Location' => location, 
          'BundleType' => bundle_type}}
      end
    end
  end
end
