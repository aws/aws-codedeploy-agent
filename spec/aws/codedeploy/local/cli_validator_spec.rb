require 'spec_helper'

require 'aws/codedeploy/local/cli_validator'

describe AWS::CodeDeploy::Local::CLIValidator do
  FAKE_DIRECTORY = "/path/directory"
  let(:validator) { AWS::CodeDeploy::Local::CLIValidator.new }

  describe 'validate' do
    context 'when type is invalid' do
      INVALID_TYPE = 'invalid-type'

      let(:args) do
        {"--type"=>INVALID_TYPE}
      end

      it 'throws a ValidationError' do
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "type #{INVALID_TYPE} is not a valid type. Must be one of #{AWS::CodeDeploy::Local::CLIValidator::VALID_TYPES.join(',')}")
      end
    end

    context 'when location is valid file' do
      VALID_FILE = "/path/test"

      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "--bundle-location"=>VALID_FILE,
         "--type"=>'tgz',
         "--help"=>false,
         "--version"=>false}
      end

      it 'returns the same arguments' do
        allow(File).to receive(:exists?).with(VALID_FILE).and_return(true)
        expect(validator.validate(args)).to equal(args)
      end
    end

    context 'when location is valid https location' do
      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "--bundle-location"=>"https://example.com/file",
         "--type"=>'tgz',
         "--help"=>false,
         "--version"=>false}
      end

      it 'returns the same arguments' do
        expect(validator.validate(args)).to equal(args)
      end
    end

    context 'when location is not a valid uri' do
      INVALID_URI = "https://invalidurl.com/file[/].html"

      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "--bundle-location"=>INVALID_URI,
         "--type"=>'tgz',
         "--help"=>false,
         "--version"=>false}
      end

      it 'throws a ValidationError' do
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "location #{INVALID_URI} is not a valid uri")
      end
    end

    context 'when location url is http' do
      HTTP_URL = "http://example.com/file"

      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "--bundle-location"=>HTTP_URL,
         "--type"=>'tgz',
         "--help"=>false,
         "--version"=>false}
      end

      it 'throws a ValidationError since unencrypted urls are not supported' do
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "location #{HTTP_URL} cannot be http, only encrypted (https) url endpoints supported")
      end
    end

    context 'when location is directory but appspec is missing' do
      let(:args) do
        {"--bundle-location"=>FAKE_DIRECTORY,
         "--type"=>'directory'}
      end

      it 'throws a ValidationError' do
        allow(File).to receive(:exists?).with(FAKE_DIRECTORY).and_return(true)
        allow(File).to receive(:directory?).with(FAKE_DIRECTORY).and_return(true)
        expect(File).to receive(:exists?).with("#{FAKE_DIRECTORY}/appspec.yml").and_return(false)
        expect(File).to receive(:exists?).with("#{FAKE_DIRECTORY}/appspec.yaml").and_return(false)
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "Expecting appspec file at location #{FAKE_DIRECTORY}/appspec.yml or #{FAKE_DIRECTORY}/appspec.yaml but it is not found there. Please either run the CLI from within a directory containing the appspec.yml or appspec.yaml file or specify a bundle location containing an appspec.yml or appspec.yaml file in its root directory")
      end
    end

    context 'when location is directory and --appspec-filename is specified (but not existing)' do
      let(:args) do
        {"--bundle-location"=>FAKE_DIRECTORY,
         "--type"=>'directory',
         "--appspec-filename"=>"appspec-override.yaml"}
      end

      it 'throws a ValidationError' do
        allow(File).to receive(:exists?).with(FAKE_DIRECTORY).and_return(true)
        allow(File).to receive(:directory?).with(FAKE_DIRECTORY).and_return(true)
        expect(File).to receive(:exists?).with("#{FAKE_DIRECTORY}/appspec-override.yaml").and_return(false)
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "Expecting appspec file at location #{FAKE_DIRECTORY}/appspec-override.yaml but it is not found there. Please either run the CLI from within a directory containing the appspec-override.yaml file or specify a bundle location containing an appspec-override.yaml file in its root directory")
      end
    end

    context 'when location is a file which does not exists' do
      FAKE_FILE_WHICH_DOES_NOT_EXIST = "/path/directory/file-does-not-exist.zip"

      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "--bundle-location"=>FAKE_FILE_WHICH_DOES_NOT_EXIST,
         "--type"=>'zip',
         "--help"=>false,
         "--version"=>false}
      end

      it 'throws a ValidationError' do
        allow(File).to receive(:exists?).with(FAKE_FILE_WHICH_DOES_NOT_EXIST).and_return(false)
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "location #{FAKE_FILE_WHICH_DOES_NOT_EXIST} is specified as a file or directory which does not exist")
      end
    end

    context 'when type is directory and location is a file' do
      FAKE_FILE = "/path/directory/file.zip"

      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "--bundle-location"=>FAKE_FILE,
         "--type"=>'directory',
         "--help"=>false,
         "--version"=>false}
      end

      it 'throws a ValidationError' do
        allow(File).to receive(:exists?).with(FAKE_FILE).and_return(true)
        allow(File).to receive(:file?).with(FAKE_FILE).and_return(true)
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "location #{FAKE_FILE} is specified with type directory but it is a file")
      end
    end

    context 'when type is zip or tgz and location is a directory' do
      let(:argszip) do
        {"deploy"=>true,
         "--location"=>true,
         "--bundle-location"=>FAKE_DIRECTORY,
         "--type"=>'zip',
         "--help"=>false,
         "--version"=>false}
      end

      let(:argstgz) do
        {"deploy"=>true,
         "--location"=>true,
         "--bundle-location"=>FAKE_DIRECTORY,
         "--type"=>'tgz',
         "--help"=>false,
         "--version"=>false}
      end

      it 'throws a ValidationError' do
        allow(File).to receive(:exists?).with(FAKE_DIRECTORY).and_return(true)
        allow(File).to receive(:directory?).with(FAKE_DIRECTORY).and_return(true)
        expect{validator.validate(argszip)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "location #{FAKE_DIRECTORY} is specified as a compressed local file but it is a directory")
        expect{validator.validate(argstgz)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "location #{FAKE_DIRECTORY} is specified as a compressed local file but it is a directory")
      end
    end

    context 'when previous revision event specified before DownloadBundle' do
      let(:args) do
        {"--bundle-location"=>FAKE_DIRECTORY,
         "--type"=>'directory',
         '--events'=>'ApplicationStart,DownloadBundle'
        }
      end

      it 'throws a ValidationError' do
        allow(File).to receive(:exists?).with(FAKE_DIRECTORY).and_return(true)
        allow(File).to receive(:directory?).with(FAKE_DIRECTORY).and_return(true)
        expect(File).to receive(:exists?).with("#{FAKE_DIRECTORY}/appspec.yml").and_return(true)
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "The only events that can be specified before DownloadBundle are BeforeBlockTraffic,AfterBlockTraffic,ApplicationStop. Please fix the order of your specified events: #{args['--events']}")
      end
    end

    context 'when Install specified before DownloadBundle' do
      let(:args) do
        {"--bundle-location"=>FAKE_DIRECTORY,
         "--type"=>'directory',
         '--events'=>'Install,DownloadBundle'
        }
      end

      it 'throws a ValidationError' do
        allow(File).to receive(:exists?).with(FAKE_DIRECTORY).and_return(true)
        allow(File).to receive(:directory?).with(FAKE_DIRECTORY).and_return(true)
        expect(File).to receive(:exists?).with("#{FAKE_DIRECTORY}/appspec.yml").and_return(true)
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "The only events that can be specified before DownloadBundle are BeforeBlockTraffic,AfterBlockTraffic,ApplicationStop. Please fix the order of your specified events: #{args['--events']}")
      end
    end

    context 'when previous revision event specified before Install' do
      let(:args) do
        {"--bundle-location"=>FAKE_DIRECTORY,
         "--type"=>'directory',
         '--events'=>'ApplicationStart,Install'
        }
      end

      it 'throws a ValidationError' do
        allow(File).to receive(:exists?).with(FAKE_DIRECTORY).and_return(true)
        allow(File).to receive(:directory?).with(FAKE_DIRECTORY).and_return(true)
        expect(File).to receive(:exists?).with("#{FAKE_DIRECTORY}/appspec.yml").and_return(true)
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "The only events that can be specified before Install are BeforeBlockTraffic,AfterBlockTraffic,ApplicationStop,DownloadBundle,BeforeInstall. Please fix the order of your specified events: #{args['--events']}")
      end
    end

    context 'when BeforeInstall event specified before Install' do
      let(:args) do
        {"--bundle-location"=>FAKE_DIRECTORY,
         "--type"=>'directory',
         '--events'=>'BeforeInstall,Install'
        }
      end

      it 'returns the same arguments' do
        allow(File).to receive(:exists?).with(FAKE_DIRECTORY).and_return(true)
        allow(File).to receive(:directory?).with(FAKE_DIRECTORY).and_return(true)
        expect(File).to receive(:exists?).with("#{FAKE_DIRECTORY}/appspec.yml").and_return(true)
        expect(validator.validate(args)).to equal(args)
      end
    end
  end
end
