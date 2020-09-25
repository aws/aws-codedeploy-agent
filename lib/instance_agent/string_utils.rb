module InstanceAgent
  class StringUtils

    def self.underscore(string)
      string.
          gsub(/([A-Z0-9]+)([A-Z][a-z])/, '\1_\2').
          scan(/[a-z0-9]+|\d+|[A-Z0-9]+[a-z]*/).
          join('_').downcase
    end

    def self.is_pascal_case(string)
      !!(string =~ /^([A-Z][a-z0-9]+)+/)
    end

  end
end