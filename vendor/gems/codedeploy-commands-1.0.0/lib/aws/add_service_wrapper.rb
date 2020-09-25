require 'aws-sdk-code-generator'
require 'aws-sdk-core'

module Aws

  # Registers a new service.
  #
  #     Aws.add_service('SvcName', api: '/path/to/svc.api.json')
  #
  #     Aws::SvcName::Client.new
  #     #=> #<Aws::SvcName::Client>
  #
  #     This implementation is taken from the AwsSdkRubyCodeGenWrapper:
  #     https://code.amazon.com/packages/AwsSdkRubyCodeGenWrapper/blobs/mainline/--/lib/add_service_wrapper.rb
  #
  # @param [String] svc_name The name of the service. This will also be
  #   the namespace under {Aws} unless options[:whitelabel] is true.
  #   This must be a valid constant name.
  # @option options[Required, String,Pathname,Hash] :api A a path to a valid
  #   Coral2JSON model or a hash of a parsed model.
  # @option options[Boolean, nil] :whitelabel If true do not prepend
  #   "Aws" to the generated module namespace.
  # @option options[String, nil] :core_path The path to the aws-sdk-core libs
  #   if unset it will be inferred from the currently loaded aws-sdk-core.
  # @option options[Hash,nil] :waiters
  # @option options[Hash,nil] :resources
  # @return [Module<Service>] Returns the new service module.
  def self.add_service(name, options = {})
    api_hash =
        case options[:api]
        when String,Pathname then JSON.parse(File.read(options[:api]))
        when Hash then options[:api]
        else raise ArgumentError, 'Missing or invalid api: must be a path to a ' \
        'valid Coral2JSON model or a hash of a parsed model.'
        end
    module_name = options[:whitelabel] ? name : "Aws::#{name}"
    core_path = options[:core_path] || File.dirname($LOADED_FEATURES.find { |f| f.include? 'aws-sdk-core.rb' })

    code = AwsSdkCodeGenerator::CodeBuilder.new(
        aws_sdk_core_lib_path: core_path,
        service: AwsSdkCodeGenerator::Service.new(
            name: name,
            module_name: module_name,
            api: api_hash,
            paginators: options[:paginators],
            paginators: options[:paginators],
            waiters: options[:waiters],
            resources: options[:resources],
            gem_dependencies: { 'aws-sdk-core' => '3' },
            gem_version: '1.0.0',
            )
    )
    begin
      Object.module_eval(code.source)
    rescue => err
      puts(code.source)
      raise err
    end
    Object.const_get(module_name)
  end
end
