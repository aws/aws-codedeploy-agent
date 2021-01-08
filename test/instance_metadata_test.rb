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

    context 'availability report' do
      should 'show IMDSv1 as available' do
        stub_request(:get, 'http://169.254.169.254/latest/meta-data').
          with(headers: {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}).
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

      should 'call the correct URL' do
        InstanceMetadata.host_identifier
        assert_requested(:put, 'http://169.254.169.254/latest/api/token', times: 2)
        assert_requested(:get, 'http://169.254.169.254/latest/meta-data/services/partition', times: 1)
        assert_requested(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document', times: 1)
      end

      should 'return sucessfully with a host identifier' do
        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should 'fallback to IMDSv1 when available' do
        # Fail the token get
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_return(status: 503, body: @instance_document, headers: {})

        # And expect that the V1 endpoint is available
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}).
            to_return(status: 200, body: @instance_document, headers: {})

        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should 'fallback to IMDSv1 when v2 errors out' do
        # Fail the token get
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_raise(StandardError)
        # And expect that the V1 endpoint is available
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}).
            to_return(status: 200, body: @instance_document, headers: {})

        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should 'strip whitesace in from the response body' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_return(status: 200, body: " \t#{@instance_document}   ", headers: {})
        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end
    end

    context 'getting the region' do

      should 'call the correct URL' do
        InstanceMetadata.region
        assert_requested(:put, 'http://169.254.169.254/latest/api/token', times: 1)
        assert_requested(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document', times: 1)
      end

      should 'return the region part of the AZ' do
        assert_equal("us-east-1", InstanceMetadata.region)
      end

      should 'fallback to IMDSv1 when available' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_return(status: 503, body: @instance_document, headers: {})
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}).
            to_return(status: 200, body: @instance_document, headers: {})
        assert_equal("us-east-1", InstanceMetadata.region)
      end

      should 'fallback to IMDSv1 when v2 errors out' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_raise(StandardError)
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}).
            to_return(status: 200, body: @instance_document, headers: {})
        assert_equal("us-east-1", InstanceMetadata.region)
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
        with(headers: {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}).
        to_raise(StandardError)
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
