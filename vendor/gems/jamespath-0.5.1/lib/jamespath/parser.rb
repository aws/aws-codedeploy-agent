require_relative 'tokenizer'

module Jamespath
  # # Grammar
  #
  # ```abnf
  # expression        : sub_expression | index_expression
  #                   | or_expression | identifier | '*'
  #                   | multi_select_list | multi_select_hash;
  # sub_expression    : expression '.' expression;
  # or_expression     : expression '||' expression;
  # index_expression  : expression bracket_specifier | bracket_specifier;
  # multi_select_list : '[' non_branched_expr ']';
  # multi_select_hash : '{' keyval_expr '}';
  # keyval_expr       : identifier ':' non_branched_expr;
  # non_branched_expr : identifier
  #                   | non_branched_expr '.' identifier
  #                   | non_branched_expr '[' number ']';
  # bracket_specifier : '[' number ']' | '[' '*' ']';
  # ```
  class Parser
    # Parses an expression into a set of instructions to be executed by the
    # {VM}.
    #
    # @param source [String] the expression to parse
    # @return [Array(Symbol, Object)] a set of instructions
    # @see VM
    def parse(source)
      @tokens = Tokenizer.new.tokenize(source)
      @idx = 0
      @instructions = []
      parse_expression
      @instructions
    end

    protected

    def parse_expression
      next_token! do |token|
        case token.type
        when :asterisk
          @instructions << [:get_key_all, nil]
        when :identifier, :number
          @instructions << [:get_key, token.value]
        when :lbrace # multi_select_hash
          parse_multi_select_hash
        when :lbracket # list
          parse_index_expression
        else
          unexpected
        end

        parse_sub_expression
      end
    end

    def parse_sub_expression
      next_token do |token|
        case token.type
        when :dot
          parse_expression
        when :double_pipe
          @instructions << [:ret_if_match, nil]
          parse_expression
        when :lbracket
          parse_index_expression
        else
          unexpected
        end
      end
    end

    def parse_index_expression
      next_token! do |token|
        case token.type
        when :number
          assert_next_type :rbracket
          @instructions << [:get_idx, token.value.to_i]
          parse_sub_expression
        when :asterisk
          assert_next_type :rbracket
          @instructions << [:get_idx_all, nil]
          parse_sub_expression
        when :rbracket
          @instructions << [:flatten_list, nil]
          parse_sub_expression
        end
      end
    end

    private

    def assert_next_type(type)
      next_token! do |token|
        unexpected(token) unless token.type == type
      end
    end

    def next_token!(peek = false, &block)
      yielded = false
      next_token(peek) {|token| yield(token); yielded = true }
      unexpected(nil) unless yielded
    end

    def next_token(peek = false, &block)
      if token = @tokens[@idx]
        @idx += 1 unless peek
        yield(token)
      end
    end

    def unexpected(token = @tokens[@idx-1])
      if token
        raise SyntaxError, "unexpected token #{token}"
      else
        raise SyntaxError, "unexpected end-of-input at #{@tokens.last}"
      end
    end
  end
end
