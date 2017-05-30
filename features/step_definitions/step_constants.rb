class StepConstants
  CODEDEPLOY_TEST_PREFIX = "codedeploy-agent-integ-test-" unless defined?(CODEDEPLOY_TEST_PREFIX)
  IS_WINDOWS = (RbConfig::CONFIG['host_os'] =~ /mswin|mingw|cygwin/) unless defined?(IS_WINDOWS)
  APP_BUNDLE_BUCKET_SUFFIX = IS_WINDOWS ? '-win' : '-linux' unless defined?(APP_BUNDLE_BUCKET_SUFFIX)

  #So I can grab the aws access key so the bucket is unique per account - unfortunately bucket names are global
  AwsCredentials.instance.configure
  APP_BUNDLE_BUCKET = "#{CODEDEPLOY_TEST_PREFIX}bucket-#{ENV['AWS_ACCESS_KEY_ID']}#{APP_BUNDLE_BUCKET_SUFFIX}".downcase unless defined?(APP_BUNDLE_BUCKET)
  APP_BUNDLE_KEY = 'app_bundle.zip' unless defined?(APP_BUNDLE_KEY)
  SAMPLE_APP_BUNDLE_DIRECTORY = IS_WINDOWS ? 'sample_app_bundle_windows' : 'sample_app_bundle_linux' unless defined?(SAMPLE_APP_BUNDLE_DIRECTORY)
  SAMPLE_APP_BUNDLE_FULL_PATH = "#{Dir.pwd}/features/resources/#{StepConstants::SAMPLE_APP_BUNDLE_DIRECTORY}" unless defined? SAMPLE_APP_BUNDLE_FULL_PATH
  SAMPLE_CUSTOM_EVENT_APP_BUNDLE_DIRECTORY = IS_WINDOWS ? 'sample_custom_event_app_bundle_windows' : 'sample_custom_event_app_bundle_linux' unless defined? SAMPLE_CUSTOM_EVENT_APP_BUNDLE_DIRECTORY
  SAMPLE_CUSTOM_EVENT_APP_BUNDLE_FULL_PATH = "#{Dir.pwd}/features/resources/#{StepConstants::SAMPLE_CUSTOM_EVENT_APP_BUNDLE_DIRECTORY}" unless defined? SAMPLE_CUSTOM_EVENT_APP_BUNDLE_FULL_PATH
  REGION = 'us-west-2' unless defined?(REGION)
end
