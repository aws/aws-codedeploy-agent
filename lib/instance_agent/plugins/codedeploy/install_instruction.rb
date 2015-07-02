require 'etc'
require 'fileutils'
require 'json'

module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      class InstallInstruction
        def self.generate_commands_from_file(file)
          name = File.basename(file.path)
          file = File.open(file.path, 'r')
          contents = file.read
          file.close
          if name =~ /^*-install.json/
            parse_install_commands(contents)
          elsif name =~ /^*-cleanup/
            parse_remove_commands(contents)
          end
        end

        def self.parse_install_commands(contents)
          instructions = JSON.parse(contents)['instructions']
          commands = []
          instructions.each do |mapping|
            case mapping['type']
            when "copy"
              commands << CopyCommand.new(mapping["source"], mapping["destination"])
            when "mkdir"
              commands << MakeDirectoryCommand.new(mapping["directory"])
            when "chmod"
              commands << ChangeModeCommand.new(mapping['file'], mapping['mode'])
            when "chown"
              commands << ChangeOwnerCommand.new(mapping['file'], mapping['owner'], mapping['group'])
            when "setfacl"
              commands << ChangeAclCommand.new(mapping['file'], InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AclInfo.new(mapping['acl']))
            when "semanage"
              if !mapping['context']['role'].nil?
                raise "Attempt to set role on object, not supported"
              end
              commands << ChangeContextCommand.new(mapping['file'], InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::ContextInfo.new(mapping['context']))
            else
              raise "Unknown command: #{mapping}"
            end
          end
          commands
        end

        def self.parse_remove_commands(contents)
          return [] if contents.empty?
          #remove the unfinished paths
          lines = contents.lines.to_a
          if lines.last[lines.last.length-1] != "\n"
            lines.pop
          end
          commands = []
          lines.each do |command|
            if command.start_with?("semanage\0")
              commands << RemoveContextCommand.new(command.split("\0",2)[1].strip)
            else
              commands << RemoveCommand.new(command.strip)
            end
          end
          commands.reverse
        end

        def self.generate_instructions()
          command_builder = CommandBuilder.new()
          yield(command_builder)
          command_builder
        end
      end

      class CommandBuilder
        attr_reader :command_array
        def initialize()
          @command_array = []
          @copy_targets = Hash.new
          @mkdir_targets = Set.new
          @permission_targets = Set.new
        end

        def copy(source, destination)
          destination = sanitize_dir_path(destination)
          log(:debug, "Copying #{source} to #{destination}")
          raise "Duplicate copy instruction to #{destination} from #{source} and #{@copy_targets[destination]}" if @copy_targets.has_key?(destination)
          raise "Duplicate copy instruction to #{destination} from #{source} which is already being installed as a directory" if @mkdir_targets.include?(destination)
          @command_array << CopyCommand.new(source, destination)
          @copy_targets[destination] = source
        end

        def mkdir(destination)
          destination = sanitize_dir_path(destination)
          log(:debug, "Making directory #{destination}")
          raise "Duplicate mkdir instruction for #{destination} which is already being copied from #{@copy_targets[destination]}" if @copy_targets.has_key?(destination)
          @command_array << MakeDirectoryCommand.new(destination) unless @mkdir_targets.include?(destination)
          @mkdir_targets.add(destination)
        end

        def set_permissions(object, permission)
          object = sanitize_dir_path(object)
          log(:debug, "Setting permissions on #{object}")
          raise "Duplicate permission setting instructions for #{object}" if @permission_targets.include?(object)
          @permission_targets.add(object)

          if !permission.mode.nil?
            log(:debug, "Setting mode on #{object}")
            @command_array << ChangeModeCommand.new(object, permission.mode.mode)
          end

          if !permission.acls.nil?
            log(:debug, "Setting acl on #{object}")
            @command_array << ChangeAclCommand.new(object, permission.acls)
          end

          if !permission.context.nil?
            log(:debug, "Setting context on #{object}")
            @command_array << ChangeContextCommand.new(object, permission.context)
          end

          if !permission.owner.nil? || !permission.group.nil?
            log(:debug, "Setting ownership of #{object}")
            @command_array << ChangeOwnerCommand.new(object, permission.owner, permission.group)
          end
        end

        def copying_file?(file)
          file = sanitize_dir_path(file)
          log(:debug, "Checking for #{file} in #{@copy_targets.keys.inspect}")
          @copy_targets.has_key?(file)
        end

        def making_directory?(dir)
          dir = sanitize_dir_path(dir)
          log(:debug, "Checking for #{dir} in #{@mkdir_targets.inspect}")
          @mkdir_targets.include?(dir)
        end

        def find_matches(permission)
          log(:debug, "Finding matches for #{permission.object}")
          matches = []
          if permission.type.include?("file")
            @copy_targets.keys.each do |object|
              log(:debug, "Checking #{object}")
              if (permission.matches_pattern?(object) && !permission.matches_except?(object))
                log(:debug, "Found match #{object}")
                permission.validate_file_acl(object)
                matches << object
              end
            end
          end
          if permission.type.include?("directory")
            @mkdir_targets.each do |object|
              log(:debug, "Checking #{object}")
              if (permission.matches_pattern?(object) && !permission.matches_except?(object))
                log(:debug, "Found match #{object}")
                matches << object
              end
            end
          end
          matches
        end

        def to_json
          command_json = @command_array.map(&:to_h)
          {:instructions => command_json}.to_json
        end

        def each(&block)
          @command_array.each(&block)
        end

        # Clean up explicitly since these can grow to tens of MBs if the deployment archive is large.
        def cleanup()
          @command_array.clear
          @command_array = nil
          @copy_targets.clear
          @copy_targets = nil
          @mkdir_targets.clear
          @mkdir_targets = nil
          @permission_targets.clear
          @permission_targets = nil
        end

        private
        def sanitize_dir_path(path)
          File.expand_path(path)
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

      class RemoveCommand
        def initialize(location)
          @file_path = location
        end

        def execute
          #If the file doesn't exist the command is ignored
          if File.exist?(@file_path)
            if File.directory?(@file_path)
              # TODO (AWSGLUE-713): handle the exception if the directory is non-empty;
              # this might mean the customer has put files in this directory and we should
              # probably ignore the error and move on
              FileUtils.rmdir(@file_path)
            else
              FileUtils.rm(@file_path)
            end
          end
        end
      end

      class CopyCommand
        attr_reader :destination, :source
        def initialize(source, destination)
          @source = source
          @destination = destination
        end

        def execute(cleanup_file)
          raise "File already exists at #{@destination}"  if File.exists?(@destination)
          cleanup_file.puts(@destination)
          if File.symlink?(@source)
            FileUtils.symlink(File.readlink(@source), @destination)
          else
            FileUtils.copy(@source, @destination, :preserve => true)
          end
        end

        def to_h
          {:type => :copy, :source => @source, :destination => @destination}
        end
      end

      class MakeDirectoryCommand
        def initialize(destination)
          @directory = destination
        end

        def execute(cleanup_file)
          raise "File already exists at #{@directory}" if
          File.exists?(@directory)
          FileUtils.mkdir(@directory)
          cleanup_file.puts(@directory)
        end

        def to_h
          {:type => :mkdir, :directory => @directory}
        end
      end

      class ChangeModeCommand
        def initialize(object, mode)
          @object = object
          @mode = mode
        end

        def execute(cleanup_file)
          File.chmod(@mode.to_i(8), @object)
        end

        def to_h
          {:type => :chmod, :mode => @mode, :file => @object}
        end
      end

      class ChangeAclCommand
        def initialize(object, acl)
          @object = object
          @acl = acl
        end

        def execute(cleanup_file)
          begin
            get_full_acl
            acl = @acl.get_acl.join(",")
            if !system("setfacl --set #{acl} #{@object}")
              raise "Unable to set acl correctly: setfacl --set #{acl} #{@object}, exit code: #{$?}"
            end
          ensure
            clear_full_acl
          end
        end

        def clear_full_acl
          @acl.clear_additional
        end

        def get_full_acl()
          perm = "%o" % File.stat(@object).mode
          perm = perm[-3,3]
          @acl.add_ace(":#{perm[0]}")
          @acl.add_ace("g::#{perm[1]}")
          @acl.add_ace("o::#{perm[2]}")
          if @acl.has_base_named? && !@acl.has_base_mask?
            @acl.add_ace("m::#{perm[1]}")
          end
          if @acl.has_default?
            if !@acl.has_default_user?
              @acl.add_ace("d::#{perm[0]}")
            end
            if !@acl.has_default_group?
              @acl.add_ace("d:g::#{perm[1]}")
            end
            if !@acl.has_default_other?
              @acl.add_ace("d:o:#{perm[2]}")
            end
            if @acl.has_default_named? && !@acl.has_default_mask?
              @acl.add_ace(@acl.get_default_group_ace.sub("group:","mask"))
            end
          end
        end

        def to_h
          {:type => :setfacl, :acl => @acl.get_acl, :file => @object}
        end
      end

      class ChangeOwnerCommand
        def initialize(object, owner, group)
          @object = object
          @owner = owner
          @group = group
        end

        def execute(cleanup_file)
          ownerid = Etc.getpwnam(@owner).uid if @owner
          groupid = Etc.getgrnam(@group).gid if @group
          File.chown(ownerid, groupid, @object)
        end

        def to_h
          {:type => :chown, :owner => @owner, :group => @group, :file => @object}
        end
      end

      class ChangeContextCommand
        def initialize(object, context)
          @object = object
          @context = context
        end

        def execute(cleanup_file)
          if !@context.role.nil?
            raise "Attempt to set role on object, not supported"
          end
          args = "-t #{@context.type}"
          if (!@context.user.nil?)
            args = "-s #{@context.user} " + args
          end
          if (!@context.range.nil?)
            args = args + " -r #{@context.range.get_range}"
          end

          object = File.realpath(@object)
          if !system("semanage fcontext -a #{args} #{object}")
            raise "Unable to set context: semanage fcontext -a #{args} #{object}, exit code: #{$?}"
          end
          if !system("restorecon -v #{object}")
            raise "Unable to apply context: restorcecon -v #{object}, exit code: #{$?}"
          end
          cleanup_file.puts("semanage\0#{object}")
        end

        def to_h
          {:type => :semanage, :context => {:user => @context.user, :role => @context.role, :type => @context.type, :range => @context.range.get_range}, :file => @object}
        end
      end

      class RemoveContextCommand
        def initialize(object)
          @object = object
        end

        def execute
          system("semanage fcontext -d #{@object}")
        end
      end
    end
  end
end
