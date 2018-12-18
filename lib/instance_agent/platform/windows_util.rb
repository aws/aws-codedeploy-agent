require 'archive/tar/minitar'
require 'zlib'
include Archive::Tar

module InstanceAgent

  class WindowsUtil

    def self.supported_versions()
      [0.0]
    end

    def self.supported_oses()
      ['windows']
    end

    def self.prepare_script_command(script, absolute_path)
      script_command = absolute_path
      if (absolute_path.downcase.end_with?('.ps1'))
        script_command = 'powershell.exe -ExecutionPolicy Bypass -File ' + absolute_path
      end
      script_command
    end

    def self.quit(exit_status = 1)
      exit(exit_status)
    end

    # end_with?() gives false for powershell scripts and ignores PATHEXT env variable
    def self.script_executable?(path)
      File.executable?(path) || path.downcase.end_with?('.ps1')
    end

    def self.extract_tar(bundle_file, dst)
      log(:warn, "Bundle format 'tar' not supported on Windows platforms. Bundle unpack may fail.")
      Minitar.unpack(bundle_file, dst)
    end

    def self.extract_tgz(bundle_file, dst)
      log(:warn, "Bundle format 'tgz' not supported on Windows platforms. Bundle unpack may fail.")
      compressed = Zlib::GzipReader.open(bundle_file)
      Minitar.unpack(compressed, dst)
    end

    def self.extract_zip(bundle_file, dst)
      log(:debug, "extract_zip - dst : #{dst}")
      FileUtils.mkdir_p(dst)
      working_dir = FileUtils.pwd()
      absolute_bundle_path = File.expand_path(bundle_file)
      execute_zip_command("powershell [System.Reflection.Assembly]::LoadWithPartialName(‘System.IO.Compression.FileSystem’); [System.IO.Compression.ZipFile]::ExtractToDirectory(‘#{absolute_bundle_path}’, ‘#{dst}’)")
    end 

    def self.supports_process_groups?()
      false
    end

    def self.codedeploy_version_file
      ProcessManager::Config.config[:root_dir]
    end

    def self.fallback_version_file
        File.join(ENV['PROGRAMDATA'], "Amazon/CodeDeploy")
    end
  
     # shelling out the rm folder command to native os in this case Window.
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
        output = `rd /s /q "#{dir}" 2>&1`
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
    def self.execute_zip_command(cmd)
      log(:debug, "Executing #{cmd}")

      output = `#{cmd} 2>&1`
      exit_status = $?.exitstatus

      log(:debug, "Command status: #{$?}")
      log(:debug, "Command output: #{output}")

      if exit_status != 0
        msg = "Error extracting zip archive: #{exit_status}"
        log(:debug, msg)
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
