require 'test_helper'

class LinuxUtilTest < InstanceAgentTestCase
  context 'Testing building command with sudo' do
    setup do
      @script_mock = Struct.new :sudo, :runas
    end

    should 'return command with sudo with runas user deploy' do
      mock = @script_mock.new true, "deploy"
      assert_equal 'sudo su deploy -c my_script.sh',
                   InstanceAgent::LinuxUtil.prepare_script_command(mock, "my_script.sh")
    end

    should 'return command without sudo with runas user deploy' do
      mock = @script_mock.new nil, "deploy"
      assert_equal 'su deploy -c my_script.sh',
                   InstanceAgent::LinuxUtil.prepare_script_command(mock, "my_script.sh")
    end

    should 'return command without sudo or runas user' do
      mock = @script_mock.new nil, nil
      assert_equal 'my_script.sh',
                   InstanceAgent::LinuxUtil.prepare_script_command(mock, "my_script.sh")
    end

    should 'return command with sudo' do
      mock = @script_mock.new true, nil
      assert_equal 'sudo my_script.sh',
                   InstanceAgent::LinuxUtil.prepare_script_command(mock, "my_script.sh")
    end

  end
end

