require 'test_helper'

class Daemon
  def running?
    true
  end

  def self.mainloop *args, &block
    false
  end
end

require 'wrapper/test_wrapper_winagent'

class WinAgentTestClass < InstanceAgentTestCase
  context 'Win agent shell try to start agent' do

    setup do
      ENV.expects(:[]).at_least_once.returns("")

      @fake_runner = mock()
      InstanceAgent::Plugins::CodeDeployPlugin::CommandPoller.stubs(:runner).returns(@fake_runner)

      logger_mock = mock()
      ::ProcessManager::Log.stubs(:init).returns(logger_mock)

      InstanceAgent::Config.expects(:load_config)
      InstanceAgent::Config.config.expects(:[]).with(:wait_between_runs).at_most(5).returns("0")
      InstanceAgent::Config.config.expects(:[]).at_least_once.returns("")
    end

    should 'starts succesfully' do
      @fake_runner.stubs(:run).times(2)
      FileUtils.expects(:cp_r).never
      @fake_runner.expects(:graceful_shutdown).never

      agent = InstanceAgentService.new
      agent.expects(:running?).times(3).returns(true, true, false)

      agent.service_main
    end

  end
end
