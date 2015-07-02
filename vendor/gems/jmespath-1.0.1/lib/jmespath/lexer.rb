module JMESPath
  # @api private
  class Lexer

    # @api private
    TOKEN_PATTERNS = {}

    # @api private
    TOKEN_TYPES = {}

    {
      '[a-zA-Z_][a-zA-Z_0-9]*'     => :identifier,
      '\.'                         => :dot,
      '\*'                         => :star,
      '\[\]'                       => :flatten,
      '-?\d+'                      => :number,
      '\|\|'                       => :or,
      '\|'                         => :pipe,
      '\[\?'                       => :filter,
      '\['                         => :lbracket,
      '\]'                         => :rbracket,
      '"(?:\\\\\\\\|\\\\"|[^"])*"' => :quoted_identifier,
      '`(?:\\\\\\\\|\\\\`|[^`])*`' => :literal,
      ','                          => :comma,
      ':'                          => :colon,
      '@'                          => :current,
      '&'                          => :expref,
      '\('                         => :lparen,
      '\)'                         => :rparen,
      '\{'                         => :lbrace,
      '\}'                         => :rbrace,
      '!='                         => :comparator,
      '=='                         => :comparator,
      '<='                         => :comparator,
      '>='                         => :comparator,
      '<'                          => :comparator,
      '>'                          => :comparator,
      '[ \t]'                      => :skip,
    }.each.with_index do |(pattern, type), n|
      TOKEN_PATTERNS[n] = pattern
      TOKEN_TYPES[n] = type
    end

    # @api private
    TOKEN_REGEX = /(#{TOKEN_PATTERNS.values.join(')|(')})/

    # @api private
    JSON_VALUE = /^[\["{]/

    # @api private
    JSON_NUMBER = /^\-?[0-9]*(\.[0-9]+)?([e|E][+|\-][0-9]+)?$/

    # @param [String<JMESPath>] expression
    # @return [Array<Hash>]
    def tokenize(expression)
      offset = 0
      tokens = []
      expression.scan(TOKEN_REGEX).each do |match|
        match_index = match.find_index { |token| !token.nil? }
        match_value = match[match_index]
        type = TOKEN_TYPES[match_index]
        token = Token.new(type, match_value, offset)
        if token.type != :skip
          case token.type
          when :number then token_number(token, expression, offset)
          when :literal then token_literal(token, expression, offset)
          when :quoted_identifier
            token_quoted_identifier(token, expression, offset)
          end
          tokens << token
        end
        offset += match_value.size
      end
      tokens << Token.new(:eof, nil, offset)
      unless expression.size == offset
        syntax_error('invalid expression', expression, offset) 
      end
      tokens
    end

    private

    def token_number(token, expression, offset)
      token[:value] = token[:value].to_i
    end

    def token_literal(token, expression, offset)
      token[:value] = token[:value][1..-2].lstrip.gsub('\`', '`')
      token[:value] =
        case token[:value]
        when 'true', 'false' then token[:value] == 'true'
        when 'null' then nil
        when '' then syntax_error("empty json literal", expression, offset)
        when JSON_VALUE then decode_json(token[:value], expression, offset)
        when JSON_NUMBER then decode_json(token[:value], expression, offset)
        else decode_json('"' + token[:value] + '"', expression, offset)
        end
    end

    def token_quoted_identifier(token, expression, offset)
      token[:value] = decode_json(token[:value], expression, offset)
    end

    def decode_json(json, expression, offset)
      MultiJson.load(json)
    rescue MultiJson::ParseError => e
      syntax_error(e.message, expression, offset)
    end

    def syntax_error(message, expression, offset)
      msg = message + "in #{expression.inspect} at #{offset}"
      raise Errors::SyntaxError.new(msg)
    end

  end
end
