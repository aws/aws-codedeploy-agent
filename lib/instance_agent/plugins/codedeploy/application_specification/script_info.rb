module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper Class for storing data parsed from hook script maps
        class ScriptInfo

          attr_reader :location, :runas, :timeout
          def initialize(location, opts = {})
            location = location.to_s
            if(location.empty?)
              raise AppSpecValidationException, 'Scripts need a location value'
            end
            @location = location
            @runas = opts[:runas]
            @timeout = opts[:timeout] || 3600
            @timeout = @timeout.to_i
            if(@timeout <= 0)
              raise AppSpecValidationException, 'Timeout needs to be an integer greater than 0'
            end
          end
        end

      end
    end
  end
end