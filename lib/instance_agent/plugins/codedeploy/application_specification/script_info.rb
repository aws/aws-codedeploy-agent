module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper Class for storing data parsed from hook script maps
        class ScriptInfo

          attr_reader :location, :runas, :sudo, :timeout
          def initialize(location, opts = {})
            location = location.to_s
            if(location.empty?)
              raise AppSpecValidationException, 'The deployment failed because the application specification file specifies a script with no location value. Specify the location in the hooks section of the AppSpec file, and then try again.'
            end
            @location = location
            @runas = opts[:runas]
            @sudo = opts[:sudo]
            @timeout = opts[:timeout] || 3600
            @timeout = @timeout.to_i
            if(@timeout <= 0)
              raise AppSpecValidationException, 'The deployment failed because an invalid timeout value was provided for a script in the application specification file. Make corrections in the hooks section of the AppSpec file, and then try again.'
            end
          end
        end

      end
    end
  end
end
