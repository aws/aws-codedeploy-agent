require 'strscan'

module Jamespath
  class Token < Struct.new(:type, :value, :pos)
    def inspect; "#{type}(#{value.inspect}, pos=#{pos})" end
    alias to_s inspect
  end

  class Tokenizer
    attr_reader :tokens

    TOKENS = {
      lbracket: /\[/,
      rbracket: /\]/,
      lbrace: /\{/,
      rbrace: /\}/,
      comma: /,/,
      dot: /\./,
      colon: /:/,
      double_pipe: /\|\|/,
      asterisk: /\*/,
      number: /-?[0-9]+/,
      quoted_identifier: /"([^"\\]|\\"|\\\\|\\[^"])*"/,
      identifier: /[a-zA-Z0-9_\u007E-\uFFFF]+/
    }

    def tokenize(source)
      @pos = 0
      @source = source
      @scanner = StringScanner.new(source)
      @tokens = []
      until @scanner.eos?
        @tokens << next_token
      end
      @tokens
    end

    protected

    def next_token
      @pos += @scanner.skip(/\s+/) || 0
      TOKENS.each do |type, re|
        if token = @scanner.scan(re) and token.length > 0
          pos, @pos = @pos, @pos + token.length
          if type == :quoted_identifier
            type = :identifier
            token = token[1...-1].gsub(/\\"/, '"')
          end

          return Token.new(type, token, pos)
        end
      end

      raise SyntaxError, "unexpected token at pos=#{@pos}: #{@source[@pos]}"
    end
  end
end
