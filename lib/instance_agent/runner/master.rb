# encoding: UTF-8
require 'process_manager/master'
require 'instance_metadata'

module InstanceAgent
  module Runner
    class Master < ProcessManager::Daemon::Master
    
      ChildTerminationMaxWaitTime = 80
      
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
          puts "Stopping #{description(pid)}"
          ProcessManager::Log.info("Stopping #{description(pid)}")
          begin
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
