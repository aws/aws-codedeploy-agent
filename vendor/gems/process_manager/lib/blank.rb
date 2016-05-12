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


# Taken from ActiveSupport core_ext/object/blank

class Object
  # An object is blank if it's false, empty, or a whitespace string.
  # For example, '', ' ', +nil+, [], and {} are all blank.
  #
  # This simplifies
  #
  # address.nil? || address.empty?
  #
  # to
  #
  # address.blank?
  #
  # @return [true, false]
  def blank?
    respond_to?(:empty?) ? !!empty? : !self
  end

  # An object is present if it's not blank.
  #
  # @return [true, false]
  def present?
    !blank?
  end

  # Returns the receiver if it's present otherwise returns +nil+.
  # <tt>object.presence</tt> is equivalent to
  #
  # object.present? ? object : nil
  #
  # For example, something like
  #
  # state = params[:state] if params[:state].present?
  # country = params[:country] if params[:country].present?
  # region = state || country || 'US'
  #
  # becomes
  #
  # region = params[:state].presence || params[:country].presence || 'US'
  #
  # @return [Object]
  def presence
    self if present?
  end
end

class NilClass
  # +nil+ is blank:
  #
  # nil.blank? # => true
  #
  # @return [true]
  def blank?
    true
  end
end

class FalseClass
  # +false+ is blank:
  #
  # false.blank? # => true
  #
  # @return [true]
  def blank?
    true
  end
end

class TrueClass
  # +true+ is not blank:
  #
  # true.blank? # => false
  #
  # @return [false]
  def blank?
    false
  end
end

class Array
  # An array is blank if it's empty:
  #
  # [].blank? # => true
  # [1,2,3].blank? # => false
  #
  # @return [true, false]
  alias_method :blank?, :empty?
end

class Hash
  # A hash is blank if it's empty:
  #
  # {}.blank? # => true
  # { key: 'value' }.blank? # => false
  #
  # @return [true, false]
  alias_method :blank?, :empty?
end

class String
  BLANK_RE = /\A[[:space:]]*\z/

  # A string is blank if it's empty or contains whitespaces only:
  #
  # ''.blank? # => true
  # ' '.blank? # => true
  # "\t\n\r".blank? # => true
  # ' blah '.blank? # => false
  #
  # Unicode whitespace is supported:
  #
  # "\u00a0".blank? # => true
  #
  # @return [true, false]
  def blank?
    BLANK_RE === self
  end
end

class Numeric #:nodoc:
  # No number is blank:
  #
  # 1.blank? # => false
  # 0.blank? # => false
  #
  # @return [false]
  def blank?
    false
  end
end
