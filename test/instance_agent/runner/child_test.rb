require 'test_helper'

class RunnerChildTest < InstanceAgentTestCase
  context 'The runner child' do
    setup do
      @dir = '/tmp'
      @agent = mock()
      InstanceAgent::Plugins::CodeDeployPlugin::CommandPoller.stubs(:new).returns(@agent)
      @agent.stubs(:description).returns("CommandPoller")
      InstanceAgent::Runner::Child.any_instance.stubs(:trap_signals)
      @child = InstanceAgent::Runner::Child.new(0, 777)
      ProcessManager::Log.init(File.join(@dir, 'codedeploy-agent.log'))
      ProcessManager.stubs(:set_program_name)
      InstanceAgent::Config.config[:wait_after_throttle_error] = 0
      InstanceAgent::Config.config[:wait_between_runs] = 0
      InstanceAgent::Config.config[:wait_between_spawning_children] = 0
      SimplePid.stubs(:drop)
      ProcessManager.reset_on_error_callbacks
    end

    context 'when preparing the run' do
      should 'load the correct runner' do
        assert_nothing_raised do
          @child = InstanceAgent::Runner::Child.new(0, 777)
          @child.prepare_run
          assert_equal @child.runner, @agent
        end
      end

      should 'validate the index' do
        assert_raise(ArgumentError) do
          @child = InstanceAgent::Runner::Child.new(9, 777)
          @child.prepare_run
        end

        assert_nothing_raised do
          @child = InstanceAgent::Runner::Child.new(0, 777)
          @child.prepare_run
        end
      end

      context 'sets the process description' do
        should 'set it for the running children' do
          @child.stubs(:runner).returns(runner = mock('runner'))
          runner.stubs(:description).returns 'master-process'
          assert_equal 'master-process of master 777', @child.description
        end

        should 'set it for the booting children' do
          assert_equal 'booting child', @child.description
        end
      end

      context 'handle exceptions' do
        setup do
          @child.stubs(:runner).returns(runner = mock('runner'))
          runner.stubs(:description).returns 'master-process'
        end
        should 'handle SocketErrors during the run and exit cleanly' do
          InstanceAgent::Config.config[:wait_after_connection_problem] = 0
          @child.expects(:runner).raises(SocketError)
          ::ProcessManager::Log.expects(:info)
          @child.expects(:sleep).with(0)
          @child.expects(:exit).with(1)
          @child.run
        end

        should 'handle throttling exceptions nicely' do
          @child.expects(:runner).raises(Exception, 'throttle exception')
          ::ProcessManager::Log.expects(:error)
          @child.expects(:sleep).with(0)
          @child.expects(:exit).with(1).at_least(1)

          @child.run
        end

        should 'handle other exceptions nicely' do
          @child.expects(:runner).raises(Exception, 'some exception')
          ::ProcessManager::Log.expects(:error)
          @child.expects(:exit).with(1)

          @child.run
        end
      end
    end
  end
end
