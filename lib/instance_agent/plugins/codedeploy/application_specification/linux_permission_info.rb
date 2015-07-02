module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper Class for storing data parsed from permissions list
        class LinuxPermissionInfo

          attr_reader :object, :pattern, :except, :type, :owner, :group, :mode, :acls, :context
          def initialize(object, opts = {})
            object = object.to_s
            if (object.empty?)
              raise AppSpecValidationException, 'Permission needs a object value'
            end
            @object = object
            @pattern = opts[:pattern] || "**"
            @except = opts[:except] || []
            @type = opts[:type] || ["file", "directory"]
            @owner = opts[:owner]
            @group = opts[:group]
            @mode = opts[:mode]
            @acls = opts[:acls]
            @context = opts[:context]
          end

          def validate_file_permission()
            if @type.include?("file")
              if !"**".eql?(@pattern)
                raise AppSpecValidationException, "Attempt to use pattern #{@pattern} when assigning permissions to file #{@object}"
              end
              if !@except.empty?
                raise AppSpecValidationException, "Attempt to use except #{@except} when assigning permissions to file #{@object}"
              end
            end
          end

          def validate_file_acl(object)
            if !@acls.nil?
              default_acl = @acls.get_default_ace
              if !default_acl.nil?
                raise "Attempt to set default acl #{default_acl} on file #{object}"
              end
            end
          end

          def matches_pattern?(name)
            name = name.chomp(File::SEPARATOR)
            base_object = sanitize_dir_path(@object)
            if !base_object.end_with?(File::SEPARATOR)
              base_object = base_object + File::SEPARATOR
            end
            if name.start_with?(base_object)
              if ("**".eql?(@pattern))
                return true
              end
              rel_name = name[base_object.length..name.length]
              return matches_simple_glob(rel_name, @pattern)
            end
            false
          end

          def matches_except?(name)
            name = name.chomp(File::SEPARATOR)
            base_object = sanitize_dir_path(@object)
            if !base_object.end_with?(File::SEPARATOR)
              base_object = base_object + File::SEPARATOR
            end
            if name.start_with?(base_object)
              rel_name = name[base_object.length..name.length]
              @except.each do |item|
                if matches_simple_glob(rel_name, item)
                  return true
                end
              end
            end
            false
          end

          private
          def matches_simple_glob(name, pattern)
            if name.include?(File::SEPARATOR)
              return false
            end
            options = expand(pattern.chars.entries)
            name.chars.each do |char|
              new_options = []
              options.each do |option|
                if option[0].eql?("*")
                  new_options.concat(expand(option))
                elsif option[0].eql?(char)
                  option.shift
                  new_options.concat(expand(option))
                end
              end
              options = new_options
              if (options.include?(["*"]))
                return true
              end
            end
            options.include?([])
          end

          private
          def expand(option)
            previous_option = nil
            while "*".eql?(option[0]) do
              previous_option = Array.new(option)
              option.shift
            end
            previous_option.nil? ? [option] : [previous_option, option]
          end

          private
          def sanitize_dir_path(path)
            File.expand_path(path)
          end
        end

      end
    end
  end
end