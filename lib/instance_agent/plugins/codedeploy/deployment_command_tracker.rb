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

            def self.create_ongoing_deployment_tracking_file(deployment_id)
              FileUtils.mkdir_p(deployment_dir_path())
              FileUtils.touch(deployment_event_tracking_file_path(deployment_id));    
            end
            
            def self.delete_deployment_tracking_file_if_stale?(deployment_id, timeout)
              if(Time.now - File.ctime(deployment_event_tracking_file_path(deployment_id)) > timeout)
                delete_deployment_command_tracking_file(deployment_id)
                return true;
              end
              return false;
            end
            
            def self.check_deployment_event_inprogress?
              if(File.exists?deployment_dir_path())
                return directories_and_files_inside(deployment_dir_path()).any?{|deployment_id| check_deployment_tracking_file_exist?(deployment_id)}
              else
                return false
              end    
            end

            def self.delete_deployment_command_tracking_file(deployment_id)
              ongoing_deployment_event_file_path = deployment_event_tracking_file_path(deployment_id)
                if File.exists?ongoing_deployment_event_file_path
                    File.delete(ongoing_deployment_event_file_path);
                else
                    InstanceAgent::Log.warn("the tracking file does not exist")
                end    
            end

            def self.directories_and_files_inside(directory)
              Dir.entries(directory) - %w(.. .)
            end
            
            private
            def self.deployment_dir_path
              File.join(InstanceAgent::Config.config[:root_dir], InstanceAgent::Config.config[:ongoing_deployment_tracking])
            end

            def self.check_deployment_tracking_file_exist?(deployment_id)
              File.exists?(deployment_event_tracking_file_path(deployment_id)) && !delete_deployment_tracking_file_if_stale?(deployment_id,
              DEPLOYMENT_EVENT_FILE_STALE_TIMELIMIT_SECONDS) 
            end
            
            def self.deployment_event_tracking_file_path(deployment_id)
              ongoing_deployment_file_path = File.join(deployment_dir_path(), deployment_id)
            end  
          end
        end
    end          
end