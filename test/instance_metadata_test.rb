require 'test_helper'
require 'json'

class InstanceMetadataTest < InstanceAgentTestCase

  def self.should_check_status_code(&blk)
    should 'raise unless status code is 200' do
      @response.stubs(:code).returns(503)
      @instance_doc_response.stubs(:code).returns(503)
      assert_raise(&blk)
    end
  end

  context 'The instance metadata service' do
    setup do
      region = 'us-east-1'
      account_id = '123456789012'
      instance_id = 'i-deadbeef'
      @partition = 'aws'
      @host_identifier = "arn:#{@partition}:ec2:#{region}:#{account_id}:instance/#{instance_id}"
      @instance_document = JSON.dump({"accountId" => account_id, "region" => region, "instanceId" => instance_id})
      @http = mock()
      @response = mock()
      @instance_doc_response = mock()
      @partition_response = mock()
      @partition_response.stubs(:code).returns("200")
      @response.stubs(:code).returns("200")
      @instance_doc_response.stubs(:code).returns("200")

      @http.stubs(:get).with('/latest/meta-data/services/partition').returns(@partition_response)
      @http.stubs(:get).with('/latest/meta-data/placement/availability-zone').returns(@response)
      @http.stubs(:get).with('/latest/dynamic/instance-identity/document').returns(@instance_doc_response)
      Net::HTTP.stubs(:start).yields(@http)
    end

    context 'getting the host identifier' do

      setup do
        @response.stubs(:body).returns("us-east-1a")
        @partition_response.stubs(:body).returns(@partition)
        @instance_doc_response.stubs(:body).returns(@instance_document)
      end

      should 'connect to the right host' do
        Net::HTTP.expects(:start).with('169.254.169.254', 80).yields(@http)
        InstanceMetadata.host_identifier
      end

      should 'call the correct URL' do
        @http.expects(:get).
          with("/latest/dynamic/instance-identity/document").
          returns(@instance_doc_response)
        InstanceMetadata.host_identifier
      end

      should 'return the body' do
        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should 'strip whitesace in the body' do
        @instance_doc_response.stubs(:body).returns(" \t#{@instance_document}   ")
        assert_equal(@host_identifier, InstanceMetadata.host_identifier)
      end

      should_check_status_code { InstanceMetadata.host_identifier }

    end

    context 'getting the region' do

      setup do
        @response.stubs(:body).returns("us-east-1a")
      end

      should 'connect to the right host' do
        Net::HTTP.expects(:start).with('169.254.169.254', 80).yields(@http)
        InstanceMetadata.region
      end

      should 'call the correct URL' do
        @http.expects(:get).
          with("/latest/meta-data/placement/availability-zone").
          returns(@response)
        InstanceMetadata.region
      end

      should 'return the region part of the AZ' do
        @response.stubs(:body).returns("us-east-1a")
        assert_equal("us-east-1", InstanceMetadata.region)
      end

      should 'raise an error if the response is not an AZ' do
        @response.stubs(:body).returns("foobar")
        assert_raise { InstanceMetadata.region }
      end

      should 'ignore whitespace in the body' do
        @response.stubs(:body).returns(" \tus-east-1a   ")
        assert_equal("us-east-1", InstanceMetadata.region)
      end

      should_check_status_code { InstanceMetadata.region }

    end

  end

end
