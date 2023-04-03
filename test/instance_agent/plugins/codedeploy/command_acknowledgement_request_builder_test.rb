require 'instance_agent'
require 'instance_agent/plugins/codedeploy/command_acknowledgement_request_builder'
require 'test_helper'

class CommandAcknowledgementRequestBuilderTest < Test::Unit::TestCase
  include ActiveSupport::Testing::Assertions
  include InstanceAgent::Plugins::CodeDeployPlugin

  @@MIN_ACK_TIMEOUT = 60
  @@MAX_ACK_TIMEOUT = 4200
  @@HOST_IDENTIFIER = 'i-123'
  @@DIAGNOSTICS = {:format => 'JSON', :payload => {'IsCommandNoop' => true}.to_json()}
  @@DEFAULT_REQUEST = {:diagnostics => @@DIAGNOSTICS, :host_command_identifier => @@HOST_IDENTIFIER}

  context 'The Command Acknowledgement Request Builder' do
    setup do
      @request_builder = InstanceAgent::Plugins::CodeDeployPlugin::CommandAcknowledgementRequestBuilder.new(
        stub(:info => nil, :warn => nil))
    end

    context 'nil timeout provided' do
      should 'exclude timeout' do
        assert_equal(@@DEFAULT_REQUEST, call_request_builder(nil))
      end
    end

    context 'timeout of zero provided' do
      should 'exclude timeout' do
        assert_equal(@@DEFAULT_REQUEST, call_request_builder(0))
      end
    end

    context '0 < timeout < 60' do
      should 'include timeout with value 60' do
        [1, 15, @@MIN_ACK_TIMEOUT-1].each do |timeout|
          assert_equal(build_expected_request(@@MIN_ACK_TIMEOUT), call_request_builder(timeout))
        end
      end
    end

    context '60 <= timeout <= 4200' do
      should 'include timeout as provided' do
        [@@MIN_ACK_TIMEOUT+1, 3600, @@MAX_ACK_TIMEOUT-1].each do |timeout|
          assert_equal(build_expected_request(timeout), call_request_builder(timeout))
        end
      end
    end

    context 'timeout > 4200' do
      should 'include timeout with value 4200' do
        assert_equal(build_expected_request(@@MAX_ACK_TIMEOUT), call_request_builder(@@MAX_ACK_TIMEOUT+1))
      end
    end
  end

  private

  def call_request_builder(timeout)
    @request_builder.build(@@DIAGNOSTICS, @@HOST_IDENTIFIER, timeout)
  end

  def build_expected_request(expected_timeout)
    result = @@DEFAULT_REQUEST.clone
    result[:host_command_max_duration_in_seconds] = expected_timeout

    result
  end

end
