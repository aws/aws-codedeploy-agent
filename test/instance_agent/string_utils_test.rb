require 'test_helper'

class StringUtilsTest < InstanceAgentTestCase

  def test_underscore_two_words()
    assert_equal("download_bundle", InstanceAgent::StringUtils.underscore("DownloadBundle"))
  end

  def test_underscore_already_underscored()
    assert_equal("download_bundle", InstanceAgent::StringUtils.underscore("download_bundle"))
  end

  def test_underscore_two_words_lowercase_first()
    assert_equal("download_bundle", InstanceAgent::StringUtils.underscore("downloadBundle"))
  end

  def test_underscore_one_word()
    assert_equal("install", InstanceAgent::StringUtils.underscore("Install"))
  end

  def test_underscore_three_words()
    assert_equal("after_allow_traffic", InstanceAgent::StringUtils.underscore("AfterAllowTraffic"))
  end

  def test_underscore_four_words()
    assert_equal("after_allow_test_traffic", InstanceAgent::StringUtils.underscore("AfterAllowTestTraffic"))
  end

  def test_is_camel_case_all_uppercase()
    assert_equal(false, InstanceAgent::StringUtils.is_pascal_case("DOWNLOADBUNDLE"))
  end

  def test_is_camel_case_all_lowercase()
    assert_equal(false, InstanceAgent::StringUtils.is_pascal_case("downloadbundle"))
  end

  def test_is_camel_case_first_lowercase()
    assert_equal(false, InstanceAgent::StringUtils.is_pascal_case("downloadBundle"))
  end

  def test_is_camel_case_second_uppercase()
    assert_equal(false, InstanceAgent::StringUtils.is_pascal_case("downloadbUndle"))
  end

  def test_is_camel_case_happy_case()
    assert_equal(true, InstanceAgent::StringUtils.is_pascal_case("DownloadBundle"))
  end
end