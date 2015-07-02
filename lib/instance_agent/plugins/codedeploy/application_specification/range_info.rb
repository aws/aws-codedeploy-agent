module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper Class for storing the range of a context
        class RangeInfo

          attr_reader :low_sensitivity, :high_sensitivity, :categories
          def initialize(range)
            parts = ensure_parts(range, ":")
            sensitivity_parts = ensure_parts(parts[0], "-")
            @low_sensitivity = getSensitivityNumber(sensitivity_parts[0])
            if sensitivity_parts.length == 2
              @high_sensitivity = getSensitivityNumber(sensitivity_parts[1])
              if @high_sensitivity < @low_sensitivity
                raise AppSpecValidationException, "invalid sensitivity range in #{range}"
              end
            else
              @high_sensitivity = @low_sensitivity
            end
            if parts.length == 2
              @categories = get_category_numbers(parts[1].split(","))
            end
          end

          def ensure_parts(input, split_on)
            num_parts = 1
            if input.include?(split_on)
              num_parts = 2
            end
            parts = input.split(split_on, 2)
            if parts.length != num_parts
              raise AppSpecValidationException, "invalid range part #{input}"
            end
            parts.each do |part|
              if part.nil? || part.eql?('')
                raise AppSpecValidationException, "invalid range part #{input}"
              end
            end
            parts
          end

          def getSensitivityNumber(sensitivity)
            if sensitivity.nil? || sensitivity.length < 2 || !sensitivity.start_with?('s')
              raise AppSpecValidationException, "invalid sensitivity #{sensitivity}"
            end
            s_level = sensitivity.sub('s', '')
            s_level.chars.each do |digit|
              if (digit.ord < '0'.ord) || (digit.ord > '9'.ord)
                raise AppSpecValidationException, "invalid sensitivity #{sensitivity}"
              end
            end
            s_level.to_i
          end

          def get_category_number(category)
            if category.nil? || category.length < 2 ||  !category.start_with?('c')
              raise AppSpecValidationException, "invalid category #{category}"
            end
            c_level = category.sub('c', '')
            c_level.chars.each do |digit|
              if (digit.ord < '0'.ord) || (digit.ord > '9'.ord)
                raise AppSpecValidationException, "invalid category #{category}"
              end
            end
            level = c_level.to_i
            if level > 1023
              raise AppSpecValidationException, "invalid category #{category}"
            end
            level
          end

          def get_category_range(range)
            low = get_category_number(range[0])
            high = get_category_number(range[1])
            if (high < low)
              raise AppSpecValidationException, "invalid category range #{range[0]}.#{range[1]}"
            end
            (low..high).to_a
          end

          def get_category_numbers(parts)
            temp_categories = [];
            parts.each do |part|
              if part.include? "."
                temp_categories.concat get_category_range(ensure_parts(part, "."))
              else
                temp_categories << get_category_number(part)
              end
            end
            if !temp_categories.sort!.uniq!.nil?
              raise AppSpecValidationException, "duplicate categories"
            end
            temp_categories
          end

          # format s#[-s#][:c#[.c#](,c#[.c#])*] (# means a number)
          def get_range
            range = "s" + @low_sensitivity.to_s
            if (@low_sensitivity != @high_sensitivity)
              range = range + "-s" + @high_sensitivity.to_s
            end
            if @categories
              range = range + ":"
              index = 0
              while index < @categories.length
                if (index != 0)
                  range = range + ","
                end

                low = @categories[index]
                low_index = index
                high = @categories[index]
                index += 1
                while (@categories[index] == low + (index - low_index))
                  high += 1
                  index += 1
                end

                if (low == high)
                  range = range + "c" + low.to_s
                else
                  range = range + "c" + low.to_s + ".c" + high.to_s
                end
              end
            end
            range
          end
        end

      end
    end
  end
end