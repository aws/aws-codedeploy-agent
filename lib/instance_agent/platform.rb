module InstanceAgent

  class Platform

    attr_accessor :util

    def self.util
      @util
    end

    def self.util=(klass)
      @util = klass
    end
  
  end

end
