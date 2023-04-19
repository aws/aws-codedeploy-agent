# Extracted from Kenneth Kalmer's excellent daemon-kit project
# on GitHub: http://github.com/kennethkalmer/daemon-kit
# (The MIT License)
#
# Copyright (c) 2009 Kenneth Kalmer (Internet Exchange CC, Clear Planet Information Solutions Pty Ltd)
#
# Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
# associated documentation files (the 'Software'), to deal in the Software without restriction,
# including without limitation the rights to use, copy, modify, merge, publish, distribute,
# sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in all copies or substantial
# portions of the Software.
#
# THE SOFTWARE IS PROVIDED 'AS IS', WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT
# NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
# IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
# WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH
# THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

require 'pathname'

class String

  # Assuming the string is a file or path name, convert it into an
  # absolute path.
  def to_absolute_path
    # Pathname is incompatible with Windows, but Windows doesn't have
    # real symlinks so File.expand_path is safe.
    if RUBY_PLATFORM =~ /(:?mswin|mingw)/
      File.expand_path( self )

      # Otherwise use Pathname#realpath which respects symlinks.
    else
      begin
        File.expand_path( Pathname.new( self ).realpath.to_s )
      rescue Errno::ENOENT
        File.expand_path( Pathname.new( self ).cleanpath.to_s )
      end
    end
  end
end
