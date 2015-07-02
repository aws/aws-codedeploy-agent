module JMESPath
  # @api private
  class ExprNode

    def initialize(interpreter, node)
      @interpreter = interpreter
      @node = node
    end

    attr_reader :interpreter

    attr_reader :node

  end
end
