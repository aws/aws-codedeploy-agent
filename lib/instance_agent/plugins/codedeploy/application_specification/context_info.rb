module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper Class for storing a context
        class ContextInfo

          attr_reader :user, :role, :type, :range
          def initialize(context)
            if context['type'].nil?
              raise AppSpecValidationException, "invalid context type required #{context.inspect}"
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