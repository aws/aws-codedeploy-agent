require_relative './jamespath/tokenizer'
require_relative './jamespath/parser'
require_relative './jamespath/vm'
require_relative './jamespath/version'

# {include:file:README.md}
module Jamespath
  module_function

  # Searches an object with a given JMESpath expression.
  #
  # @param query [String] the expression to search for.
  # @param object [Object] an object to search for the expression in.
  # @return [Object] the object, or list of objects, that match the expression.
  # @return [nil] if no objects matched the expression
  # @example Searching an object
  #   Jamespath.search('foo.bar', foo: {bar: 'result'}) #=> 'result'
  def search(query, object)
    compile(query).search(object)
  end

  # Compiles an expression that can be {VM#search searched}.
  #
  # @param query [String] the expression to search for.
  # @return [VM] a virtual machine object that can interpret the expression.
  # @see VM#search
  # @example Compiling an expression
  #   expr = Jamespath.compile('foo.bar')
  #   expr.search(foo: {bar: 'result1'}) #=> 'result1'
  #   expr.search(foo: {bar: 'result2'}) #=> 'result2'
  def compile(query)
    VM.new(Parser.new.parse(query))
  end
end
