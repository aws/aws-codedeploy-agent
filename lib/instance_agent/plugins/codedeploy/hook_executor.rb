require 'timeout'
require 'open3'
require 'json'
require 'fileutils'

module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      class ScriptLog
        attr_reader :log
        def append_to_log(log_entry)
          log_entry ||= ""
          @log ||= []
          @log.push(log_entry)

          index = @log.size
          remaining_buffer = 2048

          while (index > 0 && (remaining_buffer - @log[index-1].length) > 0)
            index = index - 1
            remaining_buffer = remaining_buffer - @log[index-1].length
          end

          if index > 0
            @log = @log.drop(index)
          end
        end

        def concat_log(log_entries)
          log_entries ||= []
          log_entries.each do |log_entry|
            append_to_log(log_entry)
          end
        end
      end

      class ScriptError < StandardError
        attr_reader :error_code, :script_name, :log

        SUCCEEDED_CODE = 0
        SCRIPT_MISSING_CODE = 1
        SCRIPT_EXECUTABILITY_CODE = 2
        SCRIPT_TIMED_OUT_CODE = 3
        SCRIPT_FAILED_CODE = 4
        UNKNOWN_ERROR_CODE = 5
        def initialize(error_code, script_name, log)
          @error_code = error_code
          @script_name = script_name
          @log = log
        end

        def to_json
          log = @log.log || []
          log = log.join("")
          {'error_code' => @error_code, 'script_name' => @script_name, 'message' => message, 'log' => log}.to_json
        end
      end

      class HookExecutor

        LAST_SUCCESSFUL_DEPLOYMENT = "OldOrIgnore"
        CURRENT = "New"
        def initialize(arguments = {})
          #check arguments
          raise "Lifecycle Event Required " if arguments[:lifecycle_event].nil?
          raise "Deployment ID required " if arguments[:deployment_id].nil?
          raise "Deployment Root Directory Required " if arguments[:deployment_root_dir].nil?
          raise "App Spec Path Required " if arguments[:app_spec_path].nil?
          raise "Application name required" if arguments[:application_name].nil?
          raise "Deployment Group name required" if arguments[:deployment_group_name].nil?
          @lifecycle_event = arguments[:lifecycle_event]
          @deployment_id = arguments[:deployment_id]
          @application_name = arguments[:application_name]
          @deployment_group_name = arguments[:deployment_group_name]
          select_correct_deployment_root_dir(arguments[:deployment_root_dir], arguments[:last_successful_deployment_dir])
          return if @deployment_root_dir.nil?
          @deployment_archive_dir = File.join(@deployment_root_dir, 'deployment-archive')
          @app_spec_path = arguments[:app_spec_path]
          parse_app_spec
          @hook_logging_mutex = Mutex.new
          @script_log = ScriptLog.new
          @child_envs={'LIFECYCLE_EVENT' => @lifecycle_event.to_s,
                      'DEPLOYMENT_ID'   => @deployment_id.to_s,
                      'APPLICATION_NAME' => @application_name,
                      'DEPLOYMENT_GROUP_NAME' => @deployment_group_name}

        end

        def execute
          return if @app_spec.nil?
          if (hooks = @app_spec.hooks[@lifecycle_event]) &&
          !hooks.empty?
            create_script_log_file_if_needed do |script_log_file|
              log_script("LifecycleEvent - " + @lifecycle_event + "\n", script_log_file)
              hooks.each do |script|
                if(!File.exist?(script_absolute_path(script)))
                  raise ScriptError.new(ScriptError::SCRIPT_MISSING_CODE, script.location, @script_log), 'Script does not exist at specified location: ' + script.location
                elsif(!InstanceAgent::Platform.util.script_executable?(script_absolute_path(script)))
                  log :warn, 'Script at specified location: ' + script.location + ' is not executable.  Trying to make it executable.'
                  begin
                    FileUtils.chmod("+x", script_absolute_path(script))
                  rescue
                    raise ScriptError.new(ScriptError::SCRIPT_EXECUTABILITY_CODE, script.location, @script_log), 'Unable to set script at specified location: ' + script.location + ' as executable'
                  end
                end
                begin
                  execute_script(script, script_log_file)
                rescue Timeout::Error
                  raise ScriptError.new(ScriptError::SCRIPT_TIMED_OUT_CODE, script.location, @script_log), 'Script at specified location: ' +script.location + ' failed to complete in '+script.timeout.to_s+' seconds'
                end
              end
            end
          end
          @script_log.log
        end

        private
        def execute_script(script, script_log_file)
          script_command = InstanceAgent::Platform.util.prepare_script_command(script, script_absolute_path(script))
          log_script("Script - " + script.location + "\n", script_log_file)
          exit_status = 1
          signal = nil

          if !InstanceAgent::Platform.util.supports_process_groups?
            # The Windows port doesn't emulate process groups so don't try to use them here
            open3_options = {}
            signal = 'KILL' #It is up to the script to handle killing child processes it spawns.
          else
            open3_options = {:pgroup => true}
            signal = '-TERM' #kill the process group instead of pid
          end

          Open3.popen3(@child_envs, script_command, open3_options) do |stdin, stdout, stderr, wait_thr|
            stdin.close
            stdout_thread = Thread.new{stdout.each_line { |line| log_script("[stdout]" + line.to_s, script_log_file)}}
            stderr_thread = Thread.new{stderr.each_line { |line| log_script("[stderr]" + line.to_s, script_log_file)}}
            if !wait_thr.join(script.timeout)
              Process.kill(signal, wait_thr.pid)
              raise Timeout::Error
            end
            stdout_thread.join
            stderr_thread.join
            exit_status = wait_thr.value.exitstatus
          end
          if(exit_status != 0)
            script_error = 'Script at specified location: ' + script.location + ' failed with exit code ' + exit_status.to_s
            if(!script.runas.nil?)
              script_error = 'Script at specified location: ' + script.location + ' run as user ' + script.runas + ' failed with exit code ' + exit_status.to_s
            end
            raise ScriptError.new(ScriptError::SCRIPT_FAILED_CODE, script.location, @script_log), script_error
          end
        end

        private
        def create_script_log_file_if_needed
          script_log_file_location = File.join(@deployment_root_dir, 'logs/scripts.log')
          if(!File.exists?(script_log_file_location))
            unless File.directory?(File.dirname(script_log_file_location))
              FileUtils.mkdir_p(File.dirname(script_log_file_location))
            end
            script_log_file = File.open(script_log_file_location, 'w')
          else
            script_log_file = File.open(script_log_file_location, 'a')
          end
          yield(script_log_file)
        ensure
          script_log_file.close unless script_log_file.nil?
        end

        private
        def script_absolute_path(script)
          File.join(@deployment_archive_dir, script.location)
        end

        private
        def parse_app_spec
          app_spec_location = File.join(@deployment_archive_dir, @app_spec_path)
          log(:debug, "Checking for app spec in #{app_spec_location}")
          @app_spec =  ApplicationSpecification::ApplicationSpecification.parse(File.read(app_spec_location))
        end

        private
        def select_correct_deployment_root_dir(current_deployment_root_dir, last_successful_deployment_root_dir)
          @deployment_root_dir = current_deployment_root_dir
          hook_deployment_mapping = mapping_between_hooks_and_deployments
          if(hook_deployment_mapping[@lifecycle_event] == LAST_SUCCESSFUL_DEPLOYMENT && !File.exist?(File.join(@deployment_root_dir, 'deployment-archive')))
            @deployment_root_dir = last_successful_deployment_root_dir
          end
        end

        private
        def mapping_between_hooks_and_deployments
          {"BeforeELBRemove"=>LAST_SUCCESSFUL_DEPLOYMENT,
            "AfterELBRemove"=>LAST_SUCCESSFUL_DEPLOYMENT,
            "ApplicationStop"=>LAST_SUCCESSFUL_DEPLOYMENT,
            "BeforeInstall"=>CURRENT,
            "AfterInstall"=>CURRENT,
            "ApplicationStart"=>CURRENT,
            "BeforeELBAdd"=>CURRENT,
            "AfterELBAdd"=>CURRENT,
            "ValidateService"=>CURRENT}
        end

        private
        def description
          self.class.to_s
        end

        private
        def log(severity, message)
          raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
          InstanceAgent::Log.send(severity.to_sym, "#{description}: #{message}")
        end

        private
        def log_script(message, script_log_file)
          @hook_logging_mutex.synchronize do
            @script_log.append_to_log(message)
            script_log_file.write(Time.now.to_s[0..-7] + ' ' + message)
            script_log_file.flush
          end
        end
      end
    end
  end
end
