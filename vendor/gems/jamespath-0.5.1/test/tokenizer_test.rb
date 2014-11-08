require 'minitest/autorun'
require_relative '../lib/jamespath'

describe Jamespath::Tokenizer do

  it "handles tokenization errors" do
    err = nil
    begin
      Jamespath::Tokenizer.new.tokenize('foo.$%^&')
    rescue SyntaxError => e
      err = e
    end
    err.message.must_equal 'unexpected token at pos=4: $'
  end
end
