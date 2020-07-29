require 'test_helper'
require 'json'
require 'webmock/rspec'
require 'webmock/test_unit'

class InstanceMetadataTest < InstanceAgentTestCase
  include WebMock::API

  def self.should_check_status_code(&blk)
    should 'raise unless status code is 200' do
      stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
          to_return(status: 503, body: @instance_document, headers: {})
      assert_raise(&blk)
    end
  end

  context 'The instance metadata service' do
    setup do
      WebMock.disable_net_connect!(allow_localhost: true)
      region = 'us-east-1'
      account_id = '123456789012'
      instance_id = 'i-deadbeef'
      @partition = 'aws'
      @host_identifier = "arn:#{@partition}:ec2:#{region}:#{account_id}:instance/#{instance_id}"
      @instance_document = JSON.dump({"accountId" => account_id, "region" => region, "instanceId" => instance_id})
      @instance_document_region_whitespace = JSON.dump({"accountId" => account_id, "region" => " us-east-1  \t", "instanceId" => instance_id})
      @token = "mock_token"

      stub_request(:put, 'http://169.254.169.254/latest/api/token').
          with(headers: {'X-aws-ec2-metadata-token-ttl-seconds' => '21600'}).
          to_return(status: 200, body: @token, headers: {})
      stub_request(:get, 'http://169.254.169.254/latest/meta-data/services/partition').
          with(headers: {'X-aws-ec2-metadata-token' => @token}).
          to_return(status: 200, body: @partition, headers: {})
      stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
          with(headers: {'X-aws-ec2-metadata-token' => @token}).
          to_return(status: 200, body: @instance_document, headers: {})
    end

    context 'getting the host identifier' do

      should 'call the correct URL' do
        InstanceMetadata.host_identifier
        assert_requested(:put, 'http://169.254.169.254/latest/api/token', times: 4)
        assert_requested(:get, 'http://169.254.169.254/latest/meta-data/services/partition', times: 1)
        assert_requested(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document', times: 3)
      end

      should 'return the body' do
        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should 'return the body if IMDSv2 http request status code is not 200' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_return(status: 503, body: @instance_document, headers: {})
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}).
            to_return(status: 200, body: @instance_document, headers: {})
        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should 'return the body if IMDSv2 http request errors out' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_raise(StandardError)
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}).
            to_return(status: 200, body: @instance_document, headers: {})
        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should 'strip whitesace in the body' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_return(status: 200, body: " \t#{@instance_document}   ", headers: {})
        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should_check_status_code { InstanceMetadata.host_identifier }

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

      should 'return the region if IMDSv2 http request status code is not 200' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_return(status: 503, body: @instance_document, headers: {})
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}).
            to_return(status: 200, body: @instance_document, headers: {})
        assert_equal("us-east-1", InstanceMetadata.region)
      end

      should 'return the region if IMDSv2 http request errors out' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_raise(StandardError)
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'Accept'=>'*/*', 'Accept-Encoding'=>'gzip;q=1.0,deflate;q=0.6,identity;q=0.3', 'User-Agent'=>'Ruby'}).
            to_return(status: 200, body: @instance_document, headers: {})
        assert_equal("us-east-1", InstanceMetadata.region)
      end

      should 'strip whitesace in the body' do
        stub_request(:get, 'http://169.254.169.254/latest/dynamic/instance-identity/document').
            with(headers: {'X-aws-ec2-metadata-token' => @token}).
            to_return(status: 200, body: @instance_document_region_whitespace , headers: {})
        assert_equal("us-east-1", InstanceMetadata.region)
      end

      should_check_status_code { InstanceMetadata.region }

    end

  end

end
