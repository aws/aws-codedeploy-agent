module InstanceAgent
  class LinuxUtil
    def self.supported_versions()
      [0.0]
    end

    def self.supported_oses()
      ['linux']
    end

    def self.prepare_script_command(script, absolute_cmd_path)
      runas = !!script.runas
      sudo = !!script.sudo

      if runas && sudo
        return 'sudo su ' + script.runas + ' -c ' + absolute_cmd_path
      end

      if runas && !sudo
        return 'su ' + script.runas + ' -c ' + absolute_cmd_path
      end

      if !runas && sudo
        return 'sudo ' + absolute_cmd_path
      end

      # If neither sudo or runas is specified, execute the
      # command as the code deploy agent user 
      absolute_cmd_path
    end

    def self.quit()
      # Send kill signal to parent and exit
      Process.kill('TERM', Process.ppid)
    end

    def self.script_executable?(path)
      File.executable?(path)
    end

    def self.extract_tar(bundle_file, dst)
      log(:debug, "extract_tar - dst : #{dst}")
      FileUtils.mkdir_p(dst)
      working_dir = FileUtils.pwd()
      absolute_bundle_path = File.expand_path(bundle_file)
      FileUtils.cd(dst) do
        execute_tar_command("/bin/tar -xpsf #{absolute_bundle_path}")
      end
    end

    def self.extract_zip(bundle_file, dst)
      log(:debug, "extract_zip - dst : #{dst}")
      FileUtils.mkdir_p(dst)
      absolute_bundle_path = File.expand_path(bundle_file)
      execute_zip_command("unzip -qo #{absolute_bundle_path} -d #{dst}")
    end    

    def self.extract_tgz(bundle_file, dst)
      log(:debug, "extract_tgz - dst : #{dst}")
      FileUtils.mkdir_p(dst)
      working_dir = FileUtils.pwd()
      absolute_bundle_path = File.expand_path(bundle_file)
      FileUtils.cd(dst) do 
        execute_tar_command("/bin/tar -zxpsf #{absolute_bundle_path}")
      end
    end

    def self.supports_process_groups?()
      true
    end

    def self.codedeploy_version_file
      File.join(ProcessManager::Config.config[:root_dir], '..')
    end

    def self.fallback_version_file
      "/opt/codedeploy-agent"
    end

     # shelling out the rm folder command to native os in this case linux.
    def self.delete_dirs_command(dirs_to_delete)
      log(:debug,"Dirs to delete: #{dirs_to_delete}");
      for dir in dirs_to_delete do
        log(:debug,"Deleting dir: #{dir}");
        delete_folder(dir);  
      end
    end
     
    private 
    def self.delete_folder (dir)
      if dir != nil && dir != "/"
        output = `rm -rf #{dir} 2>&1`
        exit_status = $?.exitstatus
        log(:debug, "Command status: #{$?}")
        log(:debug, "Command output: #{output}")
        unless exit_status == 0
          msg = "Error deleting directories: #{exit_status}"
          log(:error, msg)
          raise msg
        end 
      else
        log(:debug, "Empty directory or a wrong directory passed,#{dir}");  
      end
    end  
    
    private
    def self.execute_tar_command(cmd)
      log(:debug, "Executing #{cmd}")

      output = `#{cmd} 2>&1`
      exit_status = $?.exitstatus

      log(:debug, "Command status: #{$?}")
      log(:debug, "Command output: #{output}")

      if exit_status != 0
        msg = "Error extracting tar archive: #{exit_status}"
        log(:error, msg)
        raise msg
      end
    end

    private
    def self.execute_zip_command(cmd)
      log(:debug, "Executing #{cmd}")

      output = `#{cmd} 2>&1`
      exit_status = $?.exitstatus

      log(:debug, "Command status: #{$?}")
      log(:debug, "Command output: #{output}")

      if exit_status != 0
        msg = "Error extracting zip archive: #{exit_status}"
        log(:error, msg)
        raise msg
      end
    end
   
    private
    def self.log(severity, message)
      raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
      InstanceAgent::Log.send(severity.to_sym, "#{self.to_s}: #{message}")
    end

  end
end
