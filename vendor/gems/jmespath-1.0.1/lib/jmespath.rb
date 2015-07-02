require 'multi_json'
require 'pathname'

module JMESPath

  autoload :CachingParser, 'jmespath/caching_parser'
  autoload :Errors, 'jmespath/errors'
  autoload :ExprNode, 'jmespath/expr_node'
  autoload :Lexer, 'jmespath/lexer'
  autoload :Parser, 'jmespath/parser'
  autoload :Runtime, 'jmespath/runtime'
  autoload :Token, 'jmespath/token'
  autoload :TokenStream, 'jmespath/token_stream'
  autoload :TreeInterpreter, 'jmespath/tree_interpreter'
  autoload :VERSION, 'jmespath/version'

  class << self


    # @param [String] expression A valid
    #   [JMESPath](https://github.com/boto/jmespath) expression.
    # @param [Hash] data
    # @return [Mixed,nil] Returns the matched values. Returns `nil` if the
    #   expression does not resolve inside `data`.
    def search(expression, data)
      data = case data
        when Hash, Struct then data # check for most common case first
        when Pathname then load_json(data)
        when IO, StringIO then MultiJson.load(data.read)
        else data
        end
      Runtime.new.search(expression, data)
    end

    # @api private
    def load_json(path)
      MultiJson.load(File.open(path, 'r', encoding: 'UTF-8') { |f| f.read })
    end

  end
end
