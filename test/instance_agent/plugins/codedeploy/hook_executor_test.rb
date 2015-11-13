require 'test_helper'
require 'stringio'
require 'fileutils'

class HookExecutorTest < InstanceAgentTestCase

  include InstanceAgent::Plugins::CodeDeployPlugin

  def create_full_hook_executor
    HookExecutor.new ({:lifecycle_event => @lifecycle_event,
                        :application_name => @application_name,
                        :deployment_id => @deployment_id,
                        :deployment_group_name => @deployment_group_name,
                        :deployment_group_id => @deployment_group_id,
                        :deployment_root_dir => @deployment_root_dir,
                        :last_successful_deployment_dir => @last_successful_deployment_dir,
                        :app_spec_path => @app_spec_path})
  end

  context "testing hook executor" do
    setup do
      @deployment_id='12345'
      @application_name='TestApplication'
      @deployment_group_name='TestDeploymentGroup'
      @deployment_group_id='foo'
      @deployment_root_dir = "deployment/root/dir"
      @last_successful_deployment_dir = "last/deployment/root/dir"
      @app_spec_path = "app_spec"
      @app_spec =  { "version" => 0.0, "os" => "linux" }
      YAML.stubs(:load).returns(@app_spec)
      @root_dir = '/tmp/codedeploy'
      logger = mock
      logger.stubs(:log)
      InstanceAgent::DeploymentLog.stubs(:instance).returns(logger)
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
                                                :deployment_root_dir => @deployment_root_dir})
          end
        end
      end

      context "all information provided" do
        setup do
          @lifecycle_event = "ValidateService"
          ApplicationSpecification.stubs(:parse)
        end

        should "parse an app spec from the current deployments directory" do
          File.expects(:read).with(File.join(@deployment_root_dir, 'deployment-archive', @app_spec_path))
          @hook_executor =  create_full_hook_executor
        end

        context "hook is before download bundle" do
          setup do
            @lifecycle_event = "ApplicationStop"
          end

          should "parse an app spec from the previous deployment's directory" do
            File.expects(:read).with(File.join(@last_successful_deployment_dir, 'deployment-archive', @app_spec_path))
            @hook_executor = create_full_hook_executor
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
          @hook_executor = create_full_hook_executor
        end

        should "do nothing" do
          @hook_executor.execute
        end
      end

      context "running with a single basic script" do
        setup do
          @app_spec = {"version" => 0.0, "os" => "linux", "hooks" => {'ValidateService'=>[{'location'=>'test'}]}}
          YAML.stubs(:load).returns(@app_spec)
          @script_location = File.join(@deployment_root_dir, 'deployment-archive', 'test')
          @hook_executor = create_full_hook_executor
        end

        context "when hook script doesn't exist" do
          setup do
            File.stubs(:exist?).with(@script_location).returns(false)
          end

          should "raise and exception" do
            assert_raised_with_message("Script does not exist at specified location: #{File.expand_path(@deployment_root_dir)}/deployment-archive/test", ScriptError)do
              @hook_executor.execute
            end
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
              assert_raised_with_message('No such file or directory - deployment/root/dir/deployment-archive/test', Errno::ENOENT) do
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
            end

            context "scripts timeout" do
              setup do
                @app_spec =  { "version" => 0.0, "os" => "linux", "hooks" => {'ValidateService'=> [{"location"=>"test", "timeout"=>"30"}]}}
                YAML.stubs(:load).returns(@app_spec)
                @hook_executor = create_full_hook_executor
                @wait_thr.stubs(:join).with(30).returns(nil)
                @wait_thr.stubs(:pid).returns(1234)
                mock_pipe = mock
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
            
            context "Scripts run with a runas" do
              setup do
                @app_spec =  { "version" => 0.0, "os" => "linux", "hooks" => {'ValidateService'=> [{"location"=>"test", "runas"=>"user"}]}}
                YAML.stubs(:load).returns(@app_spec)
                @hook_executor = create_full_hook_executor
                mock_pipe = mock
                Open3.stubs(:popen3).with(@child_env, 'su user -c ' + @script_location, :pgroup => true).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
              end

              context "scripts fail" do
                setup do
                  @value.stubs(:exitstatus).returns(1)
                end

                should "raise an exception" do
                  assert_raised_with_message('Script at specified location: test run as user user failed with exit code 1', ScriptError) do
                    @hook_executor.execute
                  end
                end
              end

              context "scripts pass" do
                setup do
                  @value.stubs(:exitstatus).returns(0)
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
                @hook_executor = create_full_hook_executor
                Open3.stubs(:popen3).with(@child_env, @script_location, :pgroup => true).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
              end

              context "Scripts fail" do
                setup do
                  @value.stubs(:exitstatus).returns(1)
                end

                should "raise an exception" do
                  assert_raised_with_message('Script at specified location: test failed with exit code 1', ScriptError) do
                    @hook_executor.execute
                  end
                end
              end

              context "Scripts pass" do
                setup do
                  @value.stubs(:exitstatus).returns(0)
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
                @hook_executor = create_full_hook_executor
                Open3.stubs(:popen3).with(@child_env, @script_location, {}).yields([@mock_pipe,@mock_pipe,@mock_pipe,@wait_thr])
                InstanceAgent::LinuxUtil.stubs(:supports_process_groups?).returns(false)
              end

              context "Scripts fail" do
                setup do
                  @value.stubs(:exitstatus).returns(1)
                end

                should "raise an exception" do
                  assert_raised_with_message('Script at specified location: test failed with exit code 1', ScriptError) do
                    @hook_executor.execute
                  end
                end
              end

              context "Scripts pass" do
                setup do
                  @value.stubs(:exitstatus).returns(0)
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
    end
  end
end
