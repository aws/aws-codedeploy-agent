# encode: UTF-8
require 'test_helper'

class InstanceAgentBaseTest < InstanceAgentTestCase
  context 'The instance agent base' do
    setup do
      @base = InstanceAgent::Agent::Base.new
      @base.stubs(:sleep).returns true
    end

    context 'have a set of public methods' do
      should 'have a class method called runner' do
        assert InstanceAgent::Agent::Base.respond_to?('runner')
      end
      should 'have a description method' do
        assert @base.respond_to?('description')
      end
      should 'have a log method' do
        assert @base.respond_to?('log')
      end
      should 'have a run method' do
        assert @base.respond_to?('run')
      end
    end

    context 'rescues exceptions when running perform' do
      setup do
        @base.stubs(:log).with { |v1, v2| v1.eql?(:debug) }
      end

      should 'rescue Aws::Errors::MissingCredentialsError' do
        @base.stubs(:perform).raises Aws::Errors::MissingCredentialsError
        @base.expects(:sleep).with(any_of(9, 10))
        @base.expects(:log).with(:error, "Missing credentials - please check if this instance was started with an IAM instance profile")
        assert_nothing_raised { @base.run }
      end

      should 'rescue Aws::Errors::ServiceError' do
        @base.stubs(:perform).raises Aws::Errors::ServiceError.new(nil, "http error")
        @base.expects(:sleep).with(any_of(9, 10))
        @base.expects(:log).with { |v1, v2| v1.eql?(:error) && v2 =~ /Cannot reach InstanceService/ }
        assert_nothing_raised { @base.run }
      end

      should 'rescue all other types of exception' do
        @base.stubs(:perform).raises Exception
        @base.expects(:sleep).with(any_of(9, 10))
        @base.expects(:log).with { |v1, v2| v1.eql?(:error) && v2 =~ /Error during perform/ }
        assert_nothing_raised { @base.run }
      end
      
      should 'back off on repeated exceptions' do
        @base.stubs(:perform).raises Exception
        @base.expects(:sleep).with(any_of(9, 10))
        @base.expects(:sleep).with(any_of(12, 13))
        @base.expects(:log).twice.with { |v1, v2| v1.eql?(:error) && v2 =~ /Error during perform/ }
        assert_nothing_raised { @base.run }
        assert_nothing_raised { @base.run }
      end
    end
  end
end
