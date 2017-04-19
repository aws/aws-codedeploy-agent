require 'spec_helper'

require 'aws/codedeploy/local/cli_validator'

describe AWS::CodeDeploy::Local::CLIValidator do
  let(:validator) { AWS::CodeDeploy::Local::CLIValidator.new }

  describe 'validate' do
    context 'when location is valid file' do
      VALID_FILE = "/path/test"

      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>VALID_FILE,
         "--type"=>true,
         "tgz"=>true,
         "zip"=>false,
         "directory"=>false,
         "--event"=>1,
         "<event>"=>["stop", "start"],
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
         "<location>"=>"https://example.com/file",
         "--type"=>true,
         "tgz"=>true,
         "zip"=>false,
         "directory"=>false,
         "--event"=>1,
         "<event>"=>["stop", "start"],
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
         "<location>"=>INVALID_URI,
         "--type"=>true,
         "tgz"=>true,
         "zip"=>false,
         "directory"=>false,
         "--event"=>1,
         "<event>"=>["stop", "start"],
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
         "<location>"=>HTTP_URL,
         "--type"=>true,
         "tgz"=>true,
         "zip"=>false,
         "directory"=>false,
         "--event"=>1,
         "<event>"=>["stop", "start"],
         "--help"=>false,
         "--version"=>false}
      end

      it 'throws a ValidationError since unencrypted urls are not supported' do
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "location #{HTTP_URL} cannot be http, only encyrpted (https) url endpoints supported")
      end
    end

    context 'when location is a file which does not exists' do
      FAKE_FILE_WHICH_DOES_NOT_EXIST = "/path/directory/file-does-not-exist.zip"

      let(:args) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>FAKE_FILE_WHICH_DOES_NOT_EXIST,
         "--type"=>true,
         "tgz"=>false,
         "zip"=>true,
         "directory"=>false,
         "--event"=>1,
         "<event>"=>["stop", "start"],
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
         "<location>"=>FAKE_FILE,
         "--type"=>true,
         "tgz"=>false,
         "zip"=>false,
         "directory"=>true,
         "--event"=>1,
         "<event>"=>["stop", "start"],
         "--help"=>false,
         "--version"=>false}
      end

      it 'throws a ValidationError' do
        allow(File).to receive(:exists?).with(FAKE_FILE).and_return(true)
        allow(File).to receive(:file?).with(FAKE_FILE).and_return(true)
        expect{validator.validate(args)}.to raise_error(AWS::CodeDeploy::Local::CLIValidator::ValidationError, "location #{FAKE_FILE} is specified as an directory local directory but it is a file")
      end
    end

    context 'when type is zip or tgz and location is a directory' do
      FAKE_DIRECTORY = "/path/directory"

      let(:argszip) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>FAKE_DIRECTORY,
         "--type"=>true,
         "tgz"=>false,
         "zip"=>true,
         "directory"=>false,
         "--event"=>1,
         "<event>"=>["stop", "start"],
         "--help"=>false,
         "--version"=>false}
      end

      let(:argstgz) do
        {"deploy"=>true,
         "--location"=>true,
         "<location>"=>FAKE_DIRECTORY,
         "--type"=>true,
         "tgz"=>true,
         "zip"=>false,
         "directory"=>false,
         "--event"=>1,
         "<event>"=>["stop", "start"],
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
  end
end
