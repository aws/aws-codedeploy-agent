class StepConstants
  Aws.config[:region] = 'us-west-2'
  # Unfortunately the agent has a bug where it doesn't let the s3 client grab the region
  # from anywhere besides the instance metadata or this environment variable
  ENV['AWS_REGION'] = Aws.config[:region]

  def self.current_aws_account
    Aws::STS::Client.new.get_caller_identity.account
  end

  CODEDEPLOY_TEST_PREFIX = "codedeploy-agent-integ-test-" unless defined?(CODEDEPLOY_TEST_PREFIX)
  IS_WINDOWS = (RbConfig::CONFIG['host_os'] =~ /mswin|mingw|cygwin/) unless defined?(IS_WINDOWS)
  APP_BUNDLE_BUCKET_SUFFIX = IS_WINDOWS ? '-win' : '-linux' unless defined?(APP_BUNDLE_BUCKET_SUFFIX)

  APP_BUNDLE_BUCKET = "#{CODEDEPLOY_TEST_PREFIX}bucket-#{StepConstants.current_aws_account}#{APP_BUNDLE_BUCKET_SUFFIX}".downcase unless defined?(APP_BUNDLE_BUCKET)
  APP_BUNDLE_KEY = 'app_bundle.zip' unless defined?(APP_BUNDLE_KEY)
  SAMPLE_APP_BUNDLE_DIRECTORY = IS_WINDOWS ? 'sample_app_bundle_windows' : 'sample_app_bundle_linux' unless defined?(SAMPLE_APP_BUNDLE_DIRECTORY)
  SAMPLE_APP_BUNDLE_FULL_PATH = "#{Dir.pwd}/features/resources/#{StepConstants::SAMPLE_APP_BUNDLE_DIRECTORY}" unless defined? SAMPLE_APP_BUNDLE_FULL_PATH
  SAMPLE_CUSTOM_EVENT_APP_BUNDLE_DIRECTORY = IS_WINDOWS ? 'sample_custom_event_app_bundle_windows' : 'sample_custom_event_app_bundle_linux' unless defined? SAMPLE_CUSTOM_EVENT_APP_BUNDLE_DIRECTORY
  SAMPLE_CUSTOM_EVENT_APP_BUNDLE_FULL_PATH = "#{Dir.pwd}/features/resources/#{StepConstants::SAMPLE_CUSTOM_EVENT_APP_BUNDLE_DIRECTORY}" unless defined? SAMPLE_CUSTOM_EVENT_APP_BUNDLE_FULL_PATH
end
