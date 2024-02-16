require 'test_helper'
require 'json'
require 'webmock/rspec'
require 'webmock/test_unit'

class InstanceMetadataTest < InstanceAgentTestCase
  include WebMock::API

  setup do
    region = 'us-east-1'
    account_id = '123456789012'
    instance_id = 'i-deadbeef'
    @partition = 'aws'
    @domain = 'amazonaws.com'
    @top_level_metadata = JSON.dump(['services','instance-id'])
    @host_identifier = "arn:#{@partition}:ec2:#{region}:#{account_id}:instance/#{instance_id}"
    @instance_document = JSON.dump({"accountId" => account_id, "region" => region, "instanceId" => instance_id})
    @instance_document_region_whitespace = JSON.dump({"accountId" => account_id, "region" => " us-east-1  \t", "instanceId" => instance_id})
    @token = "mock_token"
    @max_http_attempt_count = 3 # first attempt + 2 retries
    @imds_v1_call_header = {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}
  end

  def clean_up_test_class
    # clear @@current_imds_v2_token value from previous test
    InstanceMetadata.class_variable_set(:@@current_imds_v2_token, nil)
  end

  context 'The instance metadata service' do
    setup do
      WebMock.disable_net_connect!(allow_localhost: true)

      stub_request(:put, 'http://169.254.169.254/latest/api/token').
          with(headers: {'X-aws-ec2-metadata-token-ttl-seconds' => '21600'}).
          to_return(status: 200, body: @token, headers: {})
      stub_request(:get, 'http://169.254.169.254/latest/meta-data').
          with(headers: {'X-aws-ec2-metadata-token' => @token}).
          to_return(status: 200, body: @top_level_metadata , headers: {})
      stub_request(:get, 'http://169.254.169.254/latest/meta-data/services/partition').
          with(headers: {'X-aws-ec2-metadata-token' => @token}).
          to_return(status: 200, body: @partition, headers: {})
      stub_request(:get, 'http://169.254.169.254/latest/meta-data/services/domain').
          with(headers: {'X-aws-ec2-metadata-token' => @token}).
          to_return(status: 200, body: @domain, headers: {})
      stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
          with(headers: {'X-aws-ec2-metadata-token' => @token}).
          to_return(status: 200, body: @instance_document, headers: {})
    end

    context 'imdsv1 fallback option' do
      should 'not disable imdsv1 from default configuration' do
          assert_false(InstanceMetadata.disable_imds_v1?)
      end

      should 'not disable imdsv1 if configured' do
          InstanceAgent::Config.config[:disable_imds_v1] = true
          assert_true(InstanceMetadata.disable_imds_v1?)
      end
    end

    context 'availability report' do
      setup do
        clean_up_test_class()
      end

      should 'show IMDSv1 as available' do
        stub_request(:get, 'http://169.254.169.254/latest/meta-data').
          with(headers: @imds_v1_call_header).
          to_return(status: 200, body: @top_level_metadata , headers: {})
        assert_true(InstanceMetadata.imds_v1?)
      end

      should 'show IMDSv2 as available' do
        assert_true(InstanceMetadata.imds_v2?)
      end

      should 'show any version as available' do
        assert_true(InstanceMetadata.imds_supported?)
      end
    end

    context 'getting the host identifier' do
      setup do
        clean_up_test_class()
      end

      should 'call the correct URL but with one token retrieve call' do
        InstanceMetadata.host_identifier
        assert_requested(:put, 'http://169.254.169.254/latest/api/token', times: 1)
        assert_requested(:get, 'http://169.254.169.254/latest/meta-data/services/partition', times: 1)
        assert_requested(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document', times: 1)
      end

      should 'return sucessfully with a host identifier' do
        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should 'fallback to IMDSv1 when token path is unavailable and agent enable IMDSv1, with max retry' do
        clean_up_test_class()
        # Fail the token put
        stub_request(:put, 'http://169.254.169.254/latest/api/token').
            to_return(status: 503, body: '', headers: {})

        # And expect that the V1 endpoint is available
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: @imds_v1_call_header).
            to_return(status: 200, body: @instance_document, headers: {})
        stub_request(:get, 'http://169.254.169.254/latest/meta-data/services/partition').
            with(headers: @imds_v1_call_header).
            to_return(status: 200, body: @partition, headers: {})

        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
        # both identity_document() and partition() will retry max times given the token was not retrieved successfully before
        assert_requested(:put, 'http://169.254.169.254/latest/api/token', times: @max_http_attempt_count * 2)
        assert_requested(:get, 'http://169.254.169.254/latest/meta-data/services/partition', headers: @imds_v1_call_header, times: 1)
        assert_requested(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document', times: 1)
      end

      should 'not fallback to IMDSv1 but throw exception when token path is unavailable and disable IMDSv1, with max retry' do
        clean_up_test_class()
        # Fail the token put
        stub_request(:put, 'http://169.254.169.254/latest/api/token').
            to_return(status: 503, body: '', headers: {})

        InstanceAgent::Config.config[:disable_imds_v1] = true
        error = assert_raise do
          InstanceMetadata.host_identifier
        end
        assert_equal('HTTP error from metadata service to get imdsv2 token.', error.message)
        assert_requested(:put, 'http://169.254.169.254/latest/api/token', times: @max_http_attempt_count)
      end

      should 'strip whitesace in from the response body' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_return(status: 200, body: " \t#{@instance_document}   ", headers: {})
        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should 're-retrieve the token when got token expiration error 401' do
        clean_up_test_class()
        expired_token = "expired_token"
        stub_request(:put, 'http://169.254.169.254/latest/api/token').
          with(headers: {'X-aws-ec2-metadata-token-ttl-seconds' => '21600'}).
          to_return({status: 200, body: expired_token, headers: {}}, {status: 200, body: @token, headers: {}})

        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
          with(headers: {'X-aws-ec2-metadata-token' => expired_token}).
          to_return(status: 401, body: @instance_document, headers: {})

        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
        assert_requested(:put, 'http://169.254.169.254/latest/api/token', times: 2)
        assert_requested(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document', headers: {'X-aws-ec2-metadata-token' => expired_token}, times: @max_http_attempt_count)
        assert_requested(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document', headers: {'X-aws-ec2-metadata-token' => @token}, times: 1)
        assert_requested(:get, 'http://169.254.169.254/latest/meta-data/services/partition', headers: {'X-aws-ec2-metadata-token' => @token}, times: 1)
      end
    end

    context 'getting the region' do
      setup do
        clean_up_test_class()
      end

      should 'call the correct URL' do
        InstanceMetadata.region
        assert_requested(:put, 'http://169.254.169.254/latest/api/token', times: 1)
        assert_requested(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document', times: 1)
      end

      should 'return the region part of the AZ' do
        assert_equal("us-east-1", InstanceMetadata.region)
      end

      should 'fallback to IMDSv1 when token path is unavailable and agent enable IMDSv1, with max retry' do
        clean_up_test_class()
        # Fail the token put
        stub_request(:put, 'http://169.254.169.254/latest/api/token').
            to_return(status: 503, body: '', headers: {})

        # And expect that the V1 endpoint is available
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: @imds_v1_call_header).
            to_return(status: 200, body: @instance_document, headers: {})

        assert_equal("us-east-1", InstanceMetadata.region)
        # both identity_document() and partition() will retry max times given the token was not retrieved successfully before
        assert_requested(:put, 'http://169.254.169.254/latest/api/token', times: @max_http_attempt_count)
        assert_requested(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document', times: 1)
      end

      should 'not fallback to IMDSv1 but throw exception when token path is unavailable and disable IMDSv1, with max retry' do
        clean_up_test_class()
        # Fail the token put
        stub_request(:put, 'http://169.254.169.254/latest/api/token').
            to_return(status: 503, body: '', headers: {})

        InstanceAgent::Config.config[:disable_imds_v1] = true
        assert_equal(nil, InstanceMetadata.region)
        assert_requested(:put, 'http://169.254.169.254/latest/api/token', times: @max_http_attempt_count)
      end

      should 'strip whitesace in from the response body' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_return(status: 200, body: @instance_document_region_whitespace , headers: {})
        assert_equal("us-east-1", InstanceMetadata.region)
      end
    end

  end

  context 'The instance metadata service without access' do
    setup do
      WebMock.disable_net_connect!(allow_localhost: true)

      stub_request(:put, 'http://169.254.169.254/latest/api/token').
        with(headers: {'X-aws-ec2-metadata-token-ttl-seconds' => '21600'}).
        to_raise(StandardError)
      stub_request(:get, 'http://169.254.169.254/latest/meta-data').
        with(headers: @imds_v1_call_header).
        to_raise(StandardError)
      clean_up_test_class()
    end

    should 'show unavailable for IMDSv1' do
      assert_false(InstanceMetadata.imds_v1?)
    end

    should 'show unavailable for IMDSv2' do
      assert_false(InstanceMetadata.imds_v2?)
    end

    should 'show unavailable for any IMDS version' do
      assert_false(InstanceMetadata.imds_supported?)
    end
  end
end
