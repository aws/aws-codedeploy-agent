module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper class for storing data parsed from file maps
        class FileInfo

          attr_reader :source, :destination
          def initialize(source, destination, opts = {})
            if(source.nil?)
              raise AppSpecValidationException, 'The deployment failed because the application specification file specifies a destination file, but no source file. Update the files section of the AppSpec file, and then try again.'
            elsif (destination.nil?)
              raise AppSpecValidationException, "The deployment failed because the application specification file specifies only a source file (#{source}). Add the name of the destination file to the files section of the AppSpec file, and then try again."
            end
            @source = source
            @destination = destination
          end
        end

      end
    end
  end
end
