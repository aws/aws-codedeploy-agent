# frozen_string_literal: true
package_root = File.dirname(File.dirname(__FILE__))

require "#{package_root}/vendor/gems/codedeploy-commands-1.0.0/lib/aws/add_service_wrapper"

RSpec.describe 'add_service_wrapper' do

  # This test is taken from the AwsSdkRubyCodeGenWrapper
  # https://code.amazon.com/packages/AwsSdkRubyCodeGenWrapper/blobs/mainline/--/spec/add_service_wrapper_spec.rb
  describe '#add_service' do
    before(:all) do
      @service_file = File.expand_path('../fixtures/sample_service.json', __FILE__)
      @api = JSON.parse(File.read(@service_file))
      @svc_class = Aws.add_service('GeneratedService', api: @api)
    end

    let(:client) {Aws::GeneratedService::Client.new(stub_responses: true) }

    it 'can create a valid client' do
      expect(client).to be_instance_of(Aws::GeneratedService::Client)
    end

    it 'can create a client from the returned namespace' do
      expect(@svc_class::Client.new(stub_responses: true))
          .to be_instance_of(Aws::GeneratedService::Client)
    end

    it 'can set constants on the returned namespace' do
      @svc_class.const_set(:VERSION, '1.1.42')
      expect(Aws::GeneratedService::VERSION).to eq('1.1.42')
    end

    it 'can add plugins to the generated client' do
      class MyPlugin; end
      Aws::GeneratedService::Client.add_plugin(MyPlugin)
      expect(Aws::GeneratedService::Client.plugins).to include(MyPlugin)
    end

    it 'can generate a whitelabel (non-Aws) service' do
      Aws.add_service('MyService', api: @api, whitelabel: true)
      expect(MyService::Client.new(stub_responses: true))
          .to be_instance_of(MyService::Client)
    end

    it 'loads the model from a string path' do
      Aws.add_service('StringPathService', api: @service_file)
      expect(Aws::StringPathService::Client.new(stub_responses: true))
          .to be_instance_of(Aws::StringPathService::Client)
    end

    it 'loads the model from a PathName' do
      Aws.add_service('PathService', api: Pathname.new(@service_file))
      expect(Aws::PathService::Client.new(stub_responses: true))
          .to be_instance_of(Aws::PathService::Client)
    end

    it 'raises an ArgumentError if api is not provided' do
      expect do
        Aws.add_service('NoApiService')
      end.to raise_exception(ArgumentError)
    end
  end
end