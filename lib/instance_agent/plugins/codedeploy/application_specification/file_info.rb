module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper class for storing data parsed from file maps
        class FileInfo

          attr_reader :source, :destination
          def initialize(source, destination, opts = {})
            if(source.nil?)
              raise AppSpecValidationException, 'File needs to have a source'
            elsif (destination.nil?)
              raise AppSpecValidationException, 'File needs to have a destination'
            end
            @source = source
            @destination = destination
          end
        end

      end
    end
  end
end