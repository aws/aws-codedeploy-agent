module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      # Manages install and cleanup files.  Also generates and executes
      # install instructions based on the files section of the
      # application specification file.
      class Installer

        attr_reader :deployment_archive_dir
        attr_reader :deployment_instructions_dir
        def initialize(opts = {})
          raise "the deployment_archive_dir option is required" if
          opts[:deployment_archive_dir].nil?
          raise "the deployment_instructions_dir option is required" if
          opts[:deployment_instructions_dir].nil?

          @deployment_archive_dir = opts[:deployment_archive_dir]
          @deployment_instructions_dir = opts[:deployment_instructions_dir]
        end

        def install(deployment_group_id, application_specification)
          cleanup_file = File.join(deployment_instructions_dir, "#{deployment_group_id}-cleanup")

          if File.exists?(cleanup_file)
            commands = InstallInstruction.parse_remove_commands(File.read(cleanup_file))
            commands.each do |cmd|
              cmd.execute
            end

            commands.clear
            FileUtils.rm(cleanup_file)
          end

          instructions = generate_instructions(application_specification)

          install_file = File.join(deployment_instructions_dir, "#{deployment_group_id}-install.json")
          File.open(install_file, "w") do |f|
            f.write(instructions.to_json)
          end

          File.open(cleanup_file, "w") do |f|
            instructions.command_array.each do |cmd|
              cmd.execute(f)
            end
          end

          #Unlink references to the CommandBuilder instance that was yielded to the Proc object(code block) in generate_instructions()
          instructions.cleanup
          instructions = nil
        end

        private
        def generate_instructions(application_specification)
          InstallInstruction.generate_instructions() do |i|
            application_specification.files.each do |fi|

              absolute_source_path = File.join(deployment_archive_dir,
              fi.source)

              log(:debug, "generating instructions for copying #{fi.source} to #{fi.destination}")
              if File.directory?(absolute_source_path)
                fill_in_missing_ancestors(i, fi.destination)
                generate_directory_copy(i, absolute_source_path, fi.destination)
              else
                file_destination = File.join(fi.destination, File.basename(absolute_source_path))
                fill_in_missing_ancestors(i, file_destination)
                generate_normal_copy(i, absolute_source_path, file_destination)
              end
            end

            (application_specification.permissions || []).each do |permission|
              object = permission.object

              log(:debug, "generating instructions for setting permissions on object #{object}")
              log(:debug, "it is an existing directory - #{File.directory?(object)}")
              if i.copying_file?(object)
                if permission.type.include?("file")
                  log(:debug, "found matching file #{object} to set permissions on")
                  permission.validate_file_permission
                  permission.validate_file_acl(object)
                  i.set_permissions(object, permission)
                end
              elsif (i.making_directory?(object) || File.directory?(object))
                log(:debug, "found matching directory #{object} to search for objects to set permissions on")
                i.find_matches(permission).each do|match|
                  log(:debug, "found matching object #{match} to set permissions on")
                  i.set_permissions(match, permission)
                end
              end
            end
          end
        end

        private
        def generate_directory_copy(i, absolute_source_path, destination)
          unless File.directory?(destination)
            i.mkdir(destination)
          end

          (Dir.entries(absolute_source_path) - [".", ".."]).each do |entry|
            absolute_entry_path = File.join(absolute_source_path, entry)
            entry_destination = File.join(destination, entry)
            if File.directory?(absolute_entry_path)
              generate_directory_copy(i, absolute_entry_path, entry_destination)
            else
              generate_normal_copy(i, absolute_entry_path, entry_destination)
            end
          end
        end

        private
        def generate_normal_copy(i, absolute_source_path, destination)
          raise "File already exists at location #{destination}" if
          File.exists?(destination)

          i.copy(absolute_source_path, destination)
        end

        private
        def fill_in_missing_ancestors(i, destination)
          missing_ancestors = []
          parent_dir = File.dirname(destination)
          while !File.exists?(parent_dir) &&
            parent_dir != "." && parent_dir != "/"
            missing_ancestors.unshift(parent_dir)
            parent_dir = File.dirname(parent_dir)
          end

          missing_ancestors.each do |dir|
            i.mkdir(dir)
          end
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
      end
    end
  end
end
