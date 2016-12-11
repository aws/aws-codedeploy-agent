module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper Class for storing a context
        class ContextInfo

          attr_reader :user, :role, :type, :range
          def initialize(context)
            if context['type'].nil?
              raise AppSpecValidationException, "The deployment failed because the application specification file specifies an invalid context type (#{context.inspect}). Update the permissions section of the AppSpec file, and then try again."
            end
            @user = context['name']
            @role = nil
            @type = context['type']
            @range = context['range'].nil? ? nil : RangeInfo.new(context['range'])
          end
        end

      end
    end
  end
end
