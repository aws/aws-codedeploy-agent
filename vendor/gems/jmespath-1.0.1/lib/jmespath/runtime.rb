module JMESPath
  # @api private
  class Runtime

    # @api private
    DEFAULT_PARSER = CachingParser.new

    # Constructs a new runtime object for evaluating JMESPath expressions.
    #
    #     runtime = JMESPath::Runtime.new
    #     runtime.search(expression, data)
    #     #=> ...
    #
    # ## Caching
    #
    # When constructing a {Runtime}, the default parser caches expressions.
    # This significantly speeds up calls to {#search} multiple times
    # with the same expression but different data. To disable caching, pass
    # `:cache_expressions => false` to the constructor or pass a custom
    # `:parser`.
    #
    # @example Re-use a Runtime, caching enabled by default
    #
    #   runtime = JMESPath::Runtime.new
    #   runtime.parser
    #   #=> #<JMESPath::CachingParser ...>
    #
    # @example Disable caching
    #
    #   runtime = JMESPath::Runtime.new(cache_expressions: false)
    #   runtime.parser
    #   #=> #<JMESPath::Parser ...>
    #
    # @option options [Boolean] :cache_expressions (true) When `false`, a non
    #   caching parser will be used. When `true`, a shared instance of
    #   {CachingParser} is used.  Defaults to `true`.
    #
    # @option options [Parser,CachingParser] :parser
    #
    # @option options [Interpreter] :interpreter
    #
    def initialize(options = {})
      @parser = options[:parser] || default_parser(options)
      @interpreter = options[:interpreter] || TreeInterpreter.new
    end

    # @return [Parser, CachingParser]
    attr_reader :parser

    # @return [Interpreter]
    attr_reader :interpreter

    # @param [String<JMESPath>] expression
    # @param [Hash] data
    # @return [Mixed,nil]
    def search(expression, data)
      @interpreter.visit(@parser.parse(expression), data)
    end

    private

    def default_parser(options)
      if options[:cache_expressions] == false
        Parser.new(options)
      else
        DEFAULT_PARSER
      end
    end

  end
end
