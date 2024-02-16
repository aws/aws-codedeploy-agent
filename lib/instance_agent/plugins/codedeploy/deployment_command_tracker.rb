require 'socket'
require 'concurrent'
require 'pathname'
require 'instance_metadata'
require 'instance_agent/agent/base'
require 'fileutils'
require 'instance_agent/log'

module InstanceAgent
  module Plugins
    module CodeDeployPlugin
        class FileDoesntExistException < Exception; end
        class DeploymentCommandTracker 
            DEPLOYMENT_EVENT_FILE_STALE_TIMELIMIT_SECONDS = 86400 # 24 hour limit in secounds

            def self.create_ongoing_deployment_tracking_file(deployment_id, host_command_identifier)
              retry_interval_in_sec = [1, 2, 5]
              
              # deployment tracking file creations intermittently fails on recent windows versions
              begin
                FileUtils.mkdir_p(deployment_dir_path())
                File.write(deployment_event_tracking_file_path(deployment_id), host_command_identifier)
              rescue Errno::EACCES => error
                InstanceAgent::Log.warn("Received Errno::EACCESS when creating deployment tracking file, retrying creation")
                InstanceAgent::Log.warn(error.message)
                if delay = retry_interval_in_sec.shift
                  sleep delay
                  retry
                else
                  InstanceAgent::Log.error("Exhausted retries on creating tracking file, rethrowing exception")
                  raise
                end
              end
            end
            
            def self.delete_deployment_tracking_file_if_stale?(deployment_id, timeout)
              if(Time.now - File.mtime(deployment_event_tracking_file_path(deployment_id)) > timeout)
                delete_deployment_command_tracking_file(deployment_id)
                return true;
              end
              return false;
            end
            
            def self.check_deployment_event_inprogress?
              if(File.exist?(deployment_dir_path()))
                return directories_and_files_inside(deployment_dir_path()).any?{|deployment_id| check_if_lifecycle_event_is_stale?(deployment_id)}
              else
                return false
              end    
            end

            def self.delete_deployment_command_tracking_file(deployment_id)
              ongoing_deployment_event_file_path = deployment_event_tracking_file_path(deployment_id)
              if File.exist?(ongoing_deployment_event_file_path)
                    File.delete(ongoing_deployment_event_file_path);
                else
                    InstanceAgent::Log.warn("the tracking file does not exist")
                end    
            end

            def self.directories_and_files_inside(directory)
              Dir.entries(directory) - %w(.. .)
            end

            def self.most_recent_host_command_identifier
              # check_deployment_event_inprogress handles deleting stale files for us.
              if check_deployment_event_inprogress? then
                most_recent_id = directories_and_files_inside(deployment_dir_path()).max_by{ |filename| File.mtime(deployment_event_tracking_file_path(filename)) }
                most_recent_file = deployment_event_tracking_file_path(most_recent_id)
                return File.read(most_recent_file)
              else
                return nil
              end
            end

            def self.deployment_dir_path
              File.join(InstanceAgent::Config.config[:root_dir], InstanceAgent::Config.config[:ongoing_deployment_tracking])
            end

            def self.check_if_lifecycle_event_is_stale?(deployment_id)
              !delete_deployment_tracking_file_if_stale?(deployment_id,DEPLOYMENT_EVENT_FILE_STALE_TIMELIMIT_SECONDS) 
            end
            
            def self.deployment_event_tracking_file_path(deployment_id)
              return File.join(deployment_dir_path(), deployment_id)
            end

            def self.clean_ongoing_deployment_dir
              FileUtils.rm_r(InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.deployment_dir_path()) rescue Errno::ENOENT
            end
          end
        end
    end          
end