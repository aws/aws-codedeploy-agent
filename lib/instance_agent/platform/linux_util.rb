module InstanceAgent
  class LinuxUtil
    def self.supported_versions()
      [0.0]
    end

    def self.supported_oses()
      ['linux']
    end

    def self.prepare_script_command(script, absolute_path)
      script_command = absolute_path
      if(!script.runas.nil?)
        script_command = 'su ' + script.runas + ' -c ' + absolute_path
      end
      script_command
    end

    def self.script_executable?(path)
      File.executable?(path)
    end

    def self.extract_tar(bundle_file, dst)
      FileUtils.mkdir_p(dst)
      execute_tar_command("/bin/tar -xpsf #{bundle_file} -C #{dst}")
    end

    def self.extract_tgz(bundle_file, dst)
      FileUtils.mkdir_p(dst)
      execute_tar_command("/bin/tar -zxpsf #{bundle_file} -C #{dst}")
    end

    def self.supports_process_groups?()
      true
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
    def self.log(severity, message)
      raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
      InstanceAgent::Log.send(severity.to_sym, "#{self.to_s}: #{message}")
    end

  end
end
