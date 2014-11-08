require 'minitest/autorun'
require 'json'
require_relative '../lib/jamespath'

def compliance_test(file)
  json = File.read(File.dirname(__FILE__) + '/compliance/' + file + '.json')
  test_datas = JSON.parse(json)
  describe "Compliance for #{file}.json" do
    test_datas.each.with_index do |test_data, i|
      describe "Given case #{i+1}" do
        object = test_data['given']
        test_data['cases'].each do |test_case|
          expr, result = test_case['expression'], test_case['result']
          it "handles '#{expr}'" do
            Jamespath.search(expr, object).must_equal(result)
          end
        end
      end
    end
  end
end

describe "Compliance testing" do
  compliance_test 'basic'
  compliance_test 'escape'
  compliance_test 'ormatch'
  compliance_test 'wildcard'
  compliance_test 'indices'
end
