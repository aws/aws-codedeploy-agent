# encoding: UTF-8

# some of this extentions are used during bootstrapping and don't
# have access to i.e. activesupport
#
# Methods from ruby 1.9 can be removed after migration

# 1.9 adds realpath to resolve symlinks; 1.8 doesn't
# have this method, so we add it so we get resolved symlinks
# and compatibility
unless File.respond_to?(:realpath)
  class File #:nodoc:
    def self.realpath path
      return realpath(File.readlink(path)) if symlink?(path)
      path
    end
  end
end

# Ruby 1.8.7 does not have a 'key' method on Hash
unless Hash.new.respond_to?(:key)
  class Hash
    def key(value)
      matching = select{|k,v| v == value}
      if matching && matching[0]
        matching[0][0]
      else
        nil
      end
    end
  end
end

# taken from ActiveSupport - MIT licensed
unless Hash.new.respond_to?(:symbolize_keys!)
  class Hash
    def symbolize_keys!
      keys.each do |key|
        self[(key.to_sym rescue key) || key] = delete(key)
      end
      self
    end

    def symbolize_keys
      dup.symbolize_keys!
    end
  end
end

# taken from ActiveSupport - MIT licensed
unless String.new.respond_to?(:demodulize)
  class String
    def demodulize
      path = self.to_s
      if i = path.rindex('::')
        path[(i+2)..-1]
      else
        path
      end
    end
  end
end

# taken from ActiveSupport - MIT licensed
module Kernel
  def singleton_class
    class << self
      self
    end
  end unless respond_to?(:singleton_class)
end
