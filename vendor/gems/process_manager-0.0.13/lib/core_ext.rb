#Copyright (c) 2005-2014 David Heinemeier Hansson

#Permission is hereby granted, free of charge, to any person obtaining
#a copy of this software and associated documentation files (the
#"Software"), to deal in the Software without restriction, including
#without limitation the rights to use, copy, modify, merge, publish,
#distribute, sublicense, and/or sell copies of the Software, and to
#permit persons to whom the Software is furnished to do so, subject to
#the following conditions:

#The above copyright notice and this permission notice shall be
#included in all copies or substantial portions of the Software.

#THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
#EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
#MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
#NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
#LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
#OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
#WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

# taken from ActiveSupport core_ext

# encoding: UTF-8
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

module Kernel
  def singleton_class
    class << self
      self
    end
  end unless respond_to?(:singleton_class)
end
