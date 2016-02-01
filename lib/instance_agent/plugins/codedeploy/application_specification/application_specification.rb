module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        class AppSpecValidationException < Exception; end

        class ApplicationSpecification

          attr_reader :version, :os, :hooks, :files, :permissions
          def initialize(yaml_hash, opts = {})
            @version = parse_version(yaml_hash['version'])
            @os = parse_os(yaml_hash['os'])
            @hooks = parse_hooks(yaml_hash['hooks'] || {})
            @files = parse_files(yaml_hash['files'] || [])
            @permissions = parse_permissions(yaml_hash['permissions'] || [])
          end

          def self.parse(app_spec_string)
            new(YAML.load(app_spec_string))
          end

          private
          def supported_versions()
            [0.0]
          end

          def parse_version(version)
            if !supported_versions.include?(version)
              raise AppSpecValidationException, "unsupported version: #{version}"
            end
            version
          end

          def supported_oses()
            InstanceAgent::Platform.util.supported_oses()
          end

          def parse_os(os)
            if !supported_oses.include?(os)
              raise AppSpecValidationException, "unsupported os: #{os}"
            end
            os
          end

          def parse_files(file_map_hash)
            files = []
            #loop through hash and create fileInfo representations
            file_map_hash.each do |mapping|
              files << FileInfo.new(mapping['source'], mapping['destination'])
            end
            files
          end

          def parse_hooks(hooks_hash)
            temp_hooks_hash = Hash.new
            hooks_hash.each_pair do |hook, scripts|
              current_hook_scripts = []
              scripts.each do |script|
                if (script.has_key?('location') && !script['location'].nil?)
                  current_hook_scripts << InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::ScriptInfo.new(script['location'].to_s.strip,
                  {
                    :runas => script.has_key?('runas') && !script['runas'].nil? ? script['runas'].to_s.strip : nil,
                    :timeout => script['timeout']
                  })
                else
                  raise AppSpecValidationException, 'script provided without a location value'
                end
              end
              temp_hooks_hash[hook] = current_hook_scripts
            end
            temp_hooks_hash
          end

          def parse_permissions(permissions_list)
            permissions = []
            #loop through list and create permissionsInfo representations
            permissions_list.each do |permission|
              if !permission.has_key?('object') || permission['object'].nil?
                raise AppSpecValidationException, 'permission provided without a object value'
              end
              if @os.eql?('linux')
                permissions << InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new(permission['object'].to_s.strip,
                {
                  :pattern => ('**'.eql?(permission['pattern']) || permission['pattern'].nil?) ? '**' : parse_simple_glob(permission['pattern']),
                  :except => parse_simple_glob_list(permission['except']),
                  :type => parse_type_list(permission['type']),
                  :owner => permission['owner'],
                  :group => permission['group'],
                  :mode => parse_mode(permission['mode']),
                  :acls => parse_acl(permission['acls']),
                  :context => parse_context(permission['context'])
                })
              else
                raise AppSpecValidationException, 'permissions only supported with linux os currently'
              end
            end
            permissions
          end

          #placeholder for parsing globs: we should verify that the glob is only including what we expect.  For now just returning it as it is.
          def parse_simple_glob(glob)
            glob
          end

          def parse_simple_glob_list(glob_list)
            temp_glob_list = []
            (glob_list || []).each do |glob|
              temp_glob_list << parse_simple_glob(glob)
            end
            temp_glob_list
          end

          def supported_types
            ['file', 'directory']
          end

          def parse_type_list(type_list)
            type_list ||= supported_types
            type_list.each do |type|
              if !supported_types.include?(type)
                raise AppSpecValidationException, "assigning permissions to objects of type #{type} not supported"
              end
            end
            type_list
          end

          def parse_mode(mode)
            mode.nil? ? nil : InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::ModeInfo.new(mode)
          end

          def parse_acl(acl)
            acl.nil? ? nil : InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AclInfo.new(acl)
          end

          def parse_context(context)
            context.nil? ? nil : InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::ContextInfo.new(context)
          end
        end

      end
    end
  end
end