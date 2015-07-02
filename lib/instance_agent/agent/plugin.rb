require 'set'

module InstanceAgent
  module Agent
    module Plugin
      module PluginMethods
        def plugins
          @plugins ||= Set.new
        end

        def inherited(plugin)
          plugins << plugin
        end
      end

      def self.included(klass)
        klass.extend PluginMethods
      end
    end
  end
end
