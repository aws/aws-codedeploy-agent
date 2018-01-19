# encoding: UTF-8
require 'process_manager/master'
require 'instance_metadata'
require 'instance_agent/plugins/codedeploy/deployment_command_tracker'

module InstanceAgent
  module Runner
    class DeploymentAlreadyInProgressException < Exception;  end 
    
    class Master < ProcessManager::Daemon::Master
      ChildTerminationMaxWaitTime = 3600 #timeout of an hour
      
      def self.description(pid = $$)
        "master #{pid}"
      end

      def self.child_class
        ::InstanceAgent::Runner::Child
      end

      def self.pid_description
        ProcessManager::Config.config[:program_name]
      end

      def self.log_file
        File.join(ProcessManager::Config.config[:log_dir], "#{ProcessManager::Config.config[:program_name]}.log")
      end

      def self.pid_file
        File.join(ProcessManager::Config.config[:pid_dir], "#{ProcessManager::Config.config[:program_name]}.pid")
      end

      def stop
        if (pid = self.class.find_pid)
          puts "Checking first if a deployment is already in progress"
          ProcessManager::Log.info("Checking first if any deployment lifecycle event is in progress #{description(pid)}")
          begin
            if(InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.check_deployment_event_inprogress?)
              ProcessManager::Log.info("Master process (#{pid}) will not be shut down right now, as a deployment is already in progress")
              raise "A deployment is already in Progress",DeploymentAlreadyInProgressException
            else
              puts "Stopping #{description(pid)}"
              ProcessManager::Log.info("Stopping #{description(pid)}")
            end  
            Process.kill('TERM', pid)
          rescue Errno::ESRCH
          end
          
          begin
            Timeout.timeout(ChildTerminationMaxWaitTime) do  
              loop do
                begin
                  Process.kill(0, pid)
                  sleep(1)
                rescue Errno::ESRCH
                  break
                end
              end
            end
          rescue Timeout::Error
            puts "Child processes still running. Master going down."
            ProcessManager::Log.warn("Master process (#{pid}) going down before terminating child")
          end
        else
          puts "Nothing running that could be stopped"
        end
      end

      def kill_children(sig)
	    children.each do |index, child_pid|
          begin
            Process.kill(sig, child_pid)
          rescue Errno::ESRCH
          end
        end
    
        begin
          Timeout.timeout(ChildTerminationMaxWaitTime) do
            children.each do |index, child_pid|
              begin
                Process.wait(child_pid)
              rescue Errno::ESRCH
              end
            end
          end
        rescue Timeout::Error
          children.each do |index, child_pid|
            if ProcessManager.process_running?(child_pid)
              puts "Stopping #{ProcessManager::Config.config[:program_name]} agent(#{pid}) but child(#{child_pid}) still processing."
              ProcessManager::Log.warn("Stopping #{ProcessManager::Config.config[:program_name]} agent(#{pid}) but child(#{child_pid}) is still processing.")
            end
          end
        end

      end
      
    end
  end
end
