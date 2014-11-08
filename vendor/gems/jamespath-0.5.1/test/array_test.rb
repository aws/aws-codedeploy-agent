require 'minitest/autorun'
require_relative '../lib/jamespath'

describe "Compliance testing" do

  it 'returns an empty array from a failed [] search' do
    data = {
      'items' => [
        {
          'nestedItems' => [
          ]
        }
      ]
    }
    r = Jamespath.search('items[].nestedItems[].not.there', data)
    r.must_equal([])
  end

end
