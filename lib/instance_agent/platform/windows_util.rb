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
      Minitar.unpack(bundle_file, dst)
    end

    def self.extract_tgz(bundle_file, dst)
      compressed = Zlib::GzipReader.open(bundle_file)
      Minitar.unpack(compressed, dst)
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

    private
    def self.log(severity, message)
      raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
      InstanceAgent::Log.send(severity.to_sym, "#{self.to_s}: #{message}")
    end

  end
end
