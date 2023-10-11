require 'test_helper'
require 'stringio'
require 'fileutils'

class HookExecutorTest < InstanceAgentTestCase

  include InstanceAgent::Plugins::CodeDeployPlugin

  def create_hook_executor(revision_envs = nil)
    HookExecutor.new ({:lifecycle_event => @lifecycle_event,
                        :application_name => @application_name,
                        :deployment_id => @deployment_id,
                        :deployment_group_name => @deployment_group_name,
                        :deployment_group_id => @deployment_group_id,
                        :deployment_creator => @deployment_creator,
                        :deployment_type => @deployment_type,
                        :deployment_root_dir => @deployment_root_dir,
                        :last_successful_deployment_dir => @last_successful_deployment_dir,
                        :most_recent_deployment_dir => @most_recent_deployment_dir,
                        :app_spec_path => @app_spec_path,
                        :revision_envs => revision_envs})
  end

  context "testing hook executor" do
    setup do
      @deployment_id='12345'
      @application_name='TestApplication'
      @deployment_group_name='TestDeploymentGroup'
      @deployment_group_id='foo'
      @deployment_creator = 'User'
      @deployment_type = 'IN_PLACE'
      @deployment_root_dir = "deployment/root/dir"
      @last_successful_deployment_dir = "last/successful/deployment/root/dir"
      @most_recent_deployment_dir = "most/recent/deployment/root/dir"
      @app_spec_path = "app_spec"
      @app_spec =  { "version" => 0.0, "os" => "linux" }
      YAML.stubs(:load).returns(@app_spec)
      @root_dir = '/tmp/codedeploy'
      logger = mock
      logger.stubs(:log)
      InstanceAgent::DeploymentLog.stubs(:instance).returns(logger)
      File.stubs(:exist?).returns(false)
      File.stubs(:exist?).with(){|value| value.is_a?(String) && value.end_with?("/app_spec")}.returns(true)
    end

    context "when creating a hook command" do
      context "first deployment pre-download scripts" do
        setup do
          File.stubs(:exist?).returns(false)
          @lifecycle_event = "ApplicationStop"
        end

        should "do nothing" do
          @hook_executor = HookExecutor.new ({:lifecycle_event => @lifecycle_event,
                                              :application_name => @application_name,
                                              :deployment_id => @deployment_id,
                                              :deployment_group_name => @deployment_group_name,
                                              :deployment_group_id => @deployment_group_id,
                                              :deployment_creator => @deployment_creator,
                                              :deployment_type => @deployment_type,
                                              :deployment_root_dir => @deployment_root_dir,
                                              :app_spec_path => @app_spec_path})
        end
      end

      context "first deployment post-download scripts" do
        setup do
          File.stubs(:exist?).returns(false)
          @lifecycle_event = "ValidateService"
        end

        should "raise an error" do
          assert_raise do
            @hook_executor = HookExecutor.new ({:lifecycle_event => @lifecycle_event,
                                                :application_name => @application_name,
                                                :deployment_group_name => @deployment_group_name,
                                                :deployment_group_id => @deployment_group_id,
                                                :deployment_id => @deployment_id,
                                                :deployment_creator => @deployment_creator,
                                                :deployment_type => @deployment_type,
                                                :deployment_root_dir => @deployment_root_dir})
          end
        end
      end

      context "all information provided" do
        setup do
          @lifecycle_event = "ValidateService"
          ApplicationSpecification.stubs(:parse)
        end

        should "fail if app spec not found" do 
            File.stubs(:exist?).with(){|value| value.is_a?(String) && value.end_with?("/app_spec")}.returns(false)
            assert_raised_with_message("The CodeDeploy agent did not find an AppSpec file within the unpacked revision directory at revision-relative path \"app_spec\". The revision was unpacked to directory \"deployment/root/dir/deployment-archive\", and the AppSpec file was expected but not found at path \"deployment/root/dir/deployment-archive/app_spec\". Consult the AWS CodeDeploy Appspec documentation for more information at http://docs.aws.amazon.com/codedeploy/latest/userguide/reference-appspec-file.html", RuntimeError)do
              @hook_executor =  create_hook_executor
            end
        end

        should "parse an app spec from the current deployments directory" do
          File.expects(:read).with(File.join(@deployment_root_dir, 'deployment-archive', @app_spec_path))
          @hook_executor =  create_hook_executor
        end

        context "hook is before download bundle" do
          setup do
            @lifecycle_event = "ApplicationStop"
          end

          should "parse an app spec from the last successful deployment's directory" do
            File.expects(:read).with(File.join(@last_successful_deployment_dir, 'deployment-archive', @app_spec_path))
            @hook_executor = create_hook_executor
          end
        end

        context "hook is before block traffic" do
          setup do
            @lifecycle_event = "BeforeBlockTraffic"
          end

          should "parse an app spec from the last successful deployment's directory" do
            File.expects(:read).with(File.join(@last_successful_deployment_dir, 'deployment-archive', @app_spec_path))
            @hook_executor = create_hook_executor
          end
        end

        context "hook is before block traffic blue green rollback deployment" do
          setup do
            @deployment_creator = 'codeDeployRollback'
            @deployment_type = 'BLUE_GREEN'
            @lifecycle_event = "BeforeBlockTraffic"
          end

          should "parse an app spec from the most recent deployment's directory" do
            File.expects(:read).with(File.join(@most_recent_deployment_dir, 'deployment-archive', @app_spec_path))
            @hook_executor = create_hook_executor
          end
        end
      end
    end

    context "when executing a hook command" do
      setup do
        @lifecycle_event = "ValidateService"
        File.stubs(:read).with(File.join(@deployment_root_dir, 'deployment-archive', @app_spec_path))
        @child_env={'LIFECYCLE_EVENT' => @lifecycle_event.to_s,
                    'DEPLOYMENT_ID'   => @deployment_id.to_s,
                    'APPLICATION_NAME' => @application_name.to_s,
                    'DEPLOYMENT_GROUP_NAME' => @deployment_group_name.to_s,
                    'DEPLOYMENT_GROUP_ID' => @deployment_group_id.to_s}
      end

      context "no scripts to run for a given hook" do
        setup do
          @app_spec = {"version" => 0.0, "os" => "linux", "hooks" => {}}
          YAML.stubs(:load).returns(@app_spec)
          @hook_executor = create_hook_executor
        end

        should "do nothing" do
          @hook_executor.execute
        end

        should "be a noop command" do
          assert_true @hook_executor.is_noop?
        end

        should "have a total timeout of nil" do
          assert_nil(@hook_executor.total_timeout_for_all_scripts)
        end
      end

      context "running with a single basic script" do
        setup do
          @app_spec = {
            "version" => 0.0,
            "os" => "linux",
            "hooks" => {'ValidateService'=>[{'location'=>'test', 'timeout'=>300}]}}
          YAML.stubs(:load).returns(@app_spec)
          @script_location = File.join(@deployment_root_dir, 'deployment-archive', 'test')
          @hook_executor = create_hook_executor
        end

        should "not be a noop" do
          assert_false @hook_executor.is_noop?
        end

        should "have a total timeout of 300" do
          assert_equal 300, @hook_executor.total_timeout_for_all_scripts
        end

        context "when hook script doesn't exist" do
          setup do
            File.stubs(:exist?).with(@script_location).returns(false)
          end

          should "raise an exception" do
            assert_raised_with_message("Script does not exist at specified location: #{File.expand_path(@deployment_root_dir)}/deployment-archive/test", ScriptError)do
              @hook_executor.execute
            end
          end

          should "not be a noop" do
            assert_false @hook_executor.is_noop?
          end
        end

        context "when the file exists" do
          setup do
            File.stubs(:exist?).with(@script_location).returns(true)
          end

          context "and isn't executable" do
            setup do
              File.stubs(:executable?).with(@script_location).returns(false)
              InstanceAgent::Log.expects(:send).with(:warn, 'InstanceAgent::Plugins::CodeDeployPlugin::HookExecutor: Script at specified location: test is not executable.  Trying to make it executable.')
            end

            should "log and make the hook script executable" do
              FileUtils.expects(:chmod)#.with("+x", @script_location)
              Open3.expects(:popen3).raises(Errno::ENOENT, 'deployment/root/dir/deployment-archive/test')
              assert_raised_with_message("Script at specified location: test failed with error Errno::ENOENT with message No such file or directory - deployment/root/dir/deployment-archive/test", ScriptError) do
                 @hook_executor.execute
              end
            end

            context "and setting executable fails" do
              setup do
                FileUtils.stubs(:chmod).raises("An exception")
              end

              should "raise an exception" do
                assert_raised_with_message('Unable to set script at specified location: test as executable', ScriptError) do
                  @hook_executor.execute
                end
              end
            end
          end

          context "files are executable (both intial checks pass)" do
            setup do
              File.stubs(:executable?).with(@script_location).returns(true)
              @mock_pipe = mock
              dummy_array = mock
              @mock_pipe.stubs(:each_line).returns(dummy_array)
              @mock_pipe.stubs(:close)
              dummy_array.stubs(:each).returns(nil)
              @wait_thr = mock
              @value = mock
              @wait_thr.stubs(:value).returns(@value)
              @wait_thr.stubs(:join).returns(1000)
              @thread_joiner = mock('thread_joiner')
              @thread_joiner.stubs(:joinOrFail)
              InstanceAgent::ThreadJoiner.stubs(:new).returns(@thread_joiner)
            end

            context "extra child environment variables are added" do
              setup do
                revision_envs = {"TEST_ENVIRONMENT_VARIABLE" => "ONE", "ANOTHER_ENV_VARIABLE" => "TWO"}
                @child_env.merge!(revision_envs)
                @hook_executor = create_hook_executor(revision_envs)
              end

              should "call popen with the environment variables" do
                Open3.stubs(:popen3).with(@child_env, @script_location, :pgroup => true).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
                setup_successful_status(@value)
                @hook_executor.execute()
              end
            end

            context 'scripts fail for unknown reason' do
              setup do
                @app_spec =  { "version" => 0.0, "os" => "linux", "hooks" => {'ValidateService'=> [{"location"=>"test", "timeout"=>"30"}]}}
                YAML.stubs(:load).returns(@app_spec)
                @hook_executor = create_hook_executor
                @popen_error = Errno::ENOENT
                Open3.stubs(:popen3).with(@child_env, @script_location, :pgroup => true).raises(@popen_error, 'su')
              end

              should "raise an exception" do
                popen3_error_message = 'No such file or directory - su'
                assert_raised_with_message("Script at specified location: test failed with error #{@popen_error.to_s} with message #{popen3_error_message}", ScriptError) do
                  @hook_executor.execute
                end
              end
            end

            context "scripts timeout" do
              setup do
                @app_spec =  { "version" => 0.0, "os" => "linux", "hooks" => {'ValidateService'=> [{"location"=>"test", "timeout"=>"30"}]}}
                YAML.stubs(:load).returns(@app_spec)
                @hook_executor = create_hook_executor
                @thread_joiner.expects(:joinOrFail).with(@wait_thr).yields
                InstanceAgent::ThreadJoiner.expects(:new).with(30).returns(@thread_joiner)
                @wait_thr.stubs(:pid).returns(1234)
              end

              context "with process group support" do
                setup do
                  Open3.stubs(:popen3).with(@child_env, @script_location, :pgroup => true).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
                  InstanceAgent::LinuxUtil.stubs(:supports_process_groups?).returns(true)
                end

                should "raise an exception" do
                  Process.expects(:kill).with('-TERM', 1234)
                  assert_raised_with_message('Script at specified location: test failed to complete in 30 seconds', ScriptError) do
                    @hook_executor.execute
                  end
                end
              end

              context "without process group support" do
                setup do
                  Open3.stubs(:popen3).with(@child_env, @script_location, {}).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
                  InstanceAgent::LinuxUtil.stubs(:supports_process_groups?).returns(false)
                end

                should "raise an exception" do
                  Process.expects(:kill).with('KILL', 1234)
                  assert_raised_with_message('Script at specified location: test failed to complete in 30 seconds', ScriptError) do
                    @hook_executor.execute
                  end
                end
              end
            end

            context "scripts fail to close outputs" do
              setup do
                timeout = 144
                InstanceAgent::ThreadJoiner.expects(:new).with(timeout).returns(@thread_joiner)
                @stdout_thread = mock('stdout_thread')
                @stderr_thread = mock('stderr_thread')
                Thread.stubs(:new).returns(@stdout_thread, @stderr_thread)
                Open3.stubs(:popen3).with(@child_env, @script_location, :pgroup => true).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
                @app_spec = {"version" => 0.0, "os" => "linux", "hooks" => {'ValidateService'=>[{'location'=>'test', 'timeout'=>"#{timeout}"}]}}
                YAML.stubs(:load).returns(@app_spec)
                @hook_executor = create_hook_executor
              end

              context "STDOUT left open" do
                setup do
                  @thread_joiner.expects(:joinOrFail).with(@stdout_thread).yields
                  InstanceAgent::Log.expects(:send).with(:error, "InstanceAgent::Plugins::CodeDeployPlugin::HookExecutor: Script at specified location: test failed to close STDOUT")
                end

                should "raise an exception" do
                  assert_raised_with_message("Script at specified location: test failed to close STDOUT", ScriptError) do
                    @hook_executor.execute
                  end
                end
              end

              context "STDERR left open" do
                setup do
                  @thread_joiner.expects(:joinOrFail).with(@stderr_thread).yields
                  InstanceAgent::Log.expects(:send).with(:error, "InstanceAgent::Plugins::CodeDeployPlugin::HookExecutor: Script at specified location: test failed to close STDERR")
                end

                should "raise an exception" do
                  assert_raised_with_message("Script at specified location: test failed to close STDERR", ScriptError) do
                    @hook_executor.execute
                  end
                end
              end
            end

            context "Scripts run with a runas" do
              setup do
                @app_spec =  { "version" => 0.0, "os" => "linux", "hooks" => {'ValidateService'=> [{"location"=>"test", "runas"=>"user"}]}}
                YAML.stubs(:load).returns(@app_spec)
                @hook_executor = create_hook_executor
                mock_pipe = mock
                Open3.stubs(:popen3).with(@child_env, 'su user -c ' + @script_location, :pgroup => true).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
              end

              context "scripts fail" do
                setup do
                  setup_failure_status(@value)
                end

                should "raise an exception" do
                  assert_raised_with_message('Script at specified location: test run as user user failed with exit code 1', ScriptError) do
                    @hook_executor.execute
                  end
                end
              end

              context "scripts pass" do
                setup do
                  setup_successful_status(@value)
                end

                should "execute script with runas" do
                  Open3.expects(:popen3).with(@child_env, 'su user -c ' + @script_location, :pgroup => true).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
                  @hook_executor.execute
                end
              end
            end

            context "Scripts run without a runas" do
              setup do
                @app_spec =  { "version" => 0.0, "os" => "linux", "hooks" => {'ValidateService'=> [{"location"=>"test"}]}}
                YAML.stubs(:load).returns(@app_spec)
                @hook_executor = create_hook_executor
                Open3.stubs(:popen3).with(@child_env, @script_location, :pgroup => true).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
              end

              context "Scripts fail" do
                setup do
                  setup_failure_status(@value)
                end

                should "raise an exception" do
                  assert_raised_with_message('Script at specified location: test failed with exit code 1', ScriptError) do
                    @hook_executor.execute
                  end
                end
              end

              context "Scripts pass" do
                setup do
                  setup_successful_status(@value)
                end

                should "execute script" do
                  Open3.expects(:popen3).with(@child_env, @script_location, :pgroup => true).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
                  @hook_executor.execute
                end
              end
            end
            
            context "Scripts run without process group support" do
              setup do
                @app_spec =  { "version" => 0.0, "os" => "linux", "hooks" => {'ValidateService'=> [{"location"=>"test"}]}}
                YAML.stubs(:load).returns(@app_spec)
                @hook_executor = create_hook_executor
                Open3.stubs(:popen3).with(@child_env, @script_location, {}).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
                InstanceAgent::LinuxUtil.stubs(:supports_process_groups?).returns(false)
              end

              context "Scripts fail" do
                setup do
                  setup_failure_status(@value)
                end

                should "raise an exception" do
                  assert_raised_with_message('Script at specified location: test failed with exit code 1', ScriptError) do
                    @hook_executor.execute
                  end
                end
              end

              context "Scripts pass" do
                setup do
                  setup_successful_status(@value)
                end

                should "execute script" do
                  Open3.expects(:popen3).with(@child_env, @script_location, {}).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
                  @hook_executor.execute
                end
              end
            end
          end
        end
      end

      context "running with two scripts with timeouts" do
        setup do
          @app_spec = {
            "version" => 0.0,
            "os" => "linux",
            "hooks" => {'ValidateService'=>[
              {'location'=>'test', 'timeout'=>300},
              {'location'=>'test2', 'timeout'=>150}
            ]}
          }
          YAML.stubs(:load).returns(@app_spec)
          @script_location = File.join(@deployment_root_dir, 'deployment-archive', 'test')
          @hook_executor = create_hook_executor
        end

        should "not be a noop" do
          assert_false @hook_executor.is_noop?
        end

        should "have a total timeout of 450" do
          assert_equal 450, @hook_executor.total_timeout_for_all_scripts
        end
      end

      context "running with two scripts, one with timeout" do
        setup do
          @app_spec = {
            "version" => 0.0,
            "os" => "linux",
            "hooks" => {'ValidateService'=>[
              {'location'=>'test', 'timeout'=>300},
              {'location'=>'test2'}
            ]}
          }
          YAML.stubs(:load).returns(@app_spec)
          @script_location = File.join(@deployment_root_dir, 'deployment-archive', 'test')
          @hook_executor = create_hook_executor
        end

        should "not be a noop" do
          assert_false @hook_executor.is_noop?
        end

        should "have a total timeout of 3900" do
          assert_equal 3900, @hook_executor.total_timeout_for_all_scripts
        end
      end
    end
  end

  def setup_successful_status(value)
    value.stubs(:exitstatus).returns(0)

    # for diagnostic logging
    value.stubs(:coredump?).returns(false)
    value.stubs(:exited?).returns(true)
    value.stubs(:inspect).returns("inspect result")
    value.stubs(:pid).returns(4560)
    value.stubs(:signaled?).returns(false)
    value.stubs(:stopped?).returns(false)
    value.stubs(:stopsig).returns(nil)
    value.stubs(:success?).returns(false)
    value.stubs(:termsig).returns(nil)
    value.stubs(:to_i).returns(12)
  end

  def setup_failure_status(value)
    value.stubs(:exitstatus).returns(1)

    # for diagnostic logging
    value.stubs(:coredump?).returns(false)
    value.stubs(:exited?).returns(true)
    value.stubs(:inspect).returns("inspect result")
    value.stubs(:pid).returns(4560)
    value.stubs(:signaled?).returns(false)
    value.stubs(:stopped?).returns(false)
    value.stubs(:stopsig).returns(nil)
    value.stubs(:success?).returns(false)
    value.stubs(:termsig).returns(nil)
    value.stubs(:to_i).returns(12)
  end
end
