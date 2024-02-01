require 'test_helper'
require 'aws-sdk-core'

class FileCredentialsTest < InstanceAgentTestCase
  context 'With the file credentials' do

    access_key_id = "fake-aws-access-key-id"
    secret_access_key = "fake-aws-secret-key"
    credentials_path = "/tmp/credentials_path"
    session_token_1 = "fake-aws-session-token-1"
    session_token_2 = "fake-aws-session-token-2"
    credential_file_pattern = <<-END
[default]
aws_access_key_id = #{access_key_id}
aws_secret_access_key = #{secret_access_key}
aws_session_token = %s
END

    setup do
      File.stubs(:exist?).with(credentials_path).returns(true)
      File.stubs(:exist?).with(Not(equals(credentials_path))).returns(false)
      File.stubs(:readable?).with(credentials_path).returns(true)
      File.expects(:read).with(credentials_path).returns(credential_file_pattern % session_token_2)
      File.expects(:read).with(credentials_path).returns(credential_file_pattern % session_token_1)
    end

    should 'load and refresh the credentials from the path to SharedCredentials' do
      credentials = InstanceAgent::FileCredentials.new(credentials_path)
      assert_equal access_key_id, credentials.credentials.access_key_id
      assert_equal secret_access_key, credentials.credentials.secret_access_key
      assert_equal session_token_1, credentials.credentials.session_token
      credentials.refresh!
      assert_equal access_key_id, credentials.credentials.access_key_id
      assert_equal secret_access_key, credentials.credentials.secret_access_key
      assert_equal session_token_2, credentials.credentials.session_token
    end

    should 'set the refresh time to 30 minutes' do
      credentials = InstanceAgent::FileCredentials.new(credentials_path)
      credentials.refresh!
      # Around 30 minutes
      expected_time = Time.now + 1800
      assert_in_delta(expected_time, credentials.expiration, 5, "Expiration time did not fall within 5 seconds of expected expiration")
    end
  end

  context 'Without the file credentials' do

    credentials_path = "/tmp/invalid_credentials_path"

    setup do
      File.stubs(:exist?).with(credentials_path).returns(false)
    end

    should 'raise error when credential file is missing' do
      assert_raised_with_message("Failed to load credentials from path #{credentials_path}", RuntimeError) do
        InstanceAgent::FileCredentials.new(credentials_path)
      end
    end
  end
end
