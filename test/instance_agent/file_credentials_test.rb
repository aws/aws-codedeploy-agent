class FileCredentialsTest < InstanceAgentTestCase
  context 'The file credentials' do
    should 'pass the path to SharedCredentials' do
      credentials = InstanceAgent::FileCredentials.new("/tmp/credentials_path")
      Aws::SharedCredentials.expects(:new).with(path: "/tmp/credentials_path")
      credentials.refresh!
    end

    should 'set the refresh time to 30 minutes' do
      credentials = InstanceAgent::FileCredentials.new("/tmp/credentials_path")
      credentials.refresh!
      # Around 30 minutes
      expected_time = Time.now + 1800
      assert_in_delta(expected_time, credentials.expiration, 5, "Expiration time did not fall within 5 seconds of expected expiration")
    end
  end
end
