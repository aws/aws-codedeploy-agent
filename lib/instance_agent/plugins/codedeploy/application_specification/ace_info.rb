module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper Class for storing an ace
        class AceInfo

          attr_reader :default, :type, :name, :read, :write, :execute
          def initialize(ace, internal=false)
            @default = false
            @type = nil
            @name = ""
            parts = ace.split(":", -1).reverse
            if (parts.length < 2) || (parts.length > 4)
              raise AppSpecValidationException, "invalid acl entry #{ace}"
            end

            if (parts.length == 4)
              if !(parts[3].eql?("d") || (parts[3].eql?("default")))
                raise AppSpecValidationException, "invalid acl entry #{ace}"
              end
              @default = true
            end

            if parts.length >= 3
              if parts[2].eql?("d") || (parts[2].eql?("default"))
                if @default
                  raise AppSpecValidationException, "invalid acl entry #{ace}"
                end
                @default = true
              elsif parts[2].eql?("m") || parts[2].eql?("mask")
                @type = "mask"
              elsif parts[2].eql?("o") || parts[2].eql?("other")
                @type = "other"
              elsif parts[2].eql?("g") || parts[2].eql?("group")
                @type = "group"
              elsif parts[2].eql?("u") || parts[2].eql?("user")
                @type = "user"
              else
                raise AppSpecValidationException, "invalid acl entry #{ace}"
              end
            end

            if  parts[1].eql?("m") || parts[1].eql?("mask")
              if @type.nil?
                @type = "mask"
              else
                @name = "mask"
              end
            elsif parts[1].eql?("o") || parts[1].eql?("other")
              if @type.nil?
                @type = "other"
              else
                @name = "other"
              end
            else
              if @type.nil?
                @type = "user"
              end
              @name = parts[1]
            end

            if (@type.eql?("mask") || @type.eql?("other")) && !@name.empty?
              raise AppSpecValidationException, "invalid acl entry #{ace}"
            end
            if (!internal && !@default && !@type.eql?("mask") && @name.empty?)
              raise AppSpecValidationException, "use mode to set the base acl entry #{ace}"
            end

            perm_chars = parts[0].chars.entries
            if (perm_chars.length == 1) && (perm_chars[0].ord >= "0".ord) && (perm_chars[0].ord <= "7".ord)
              perm_bits = to_bits(perm_chars[0].to_i, 3)
              @read = (perm_bits[0] == 1)
              @write = (perm_bits[1] == 1)
              @execute = (perm_bits[2] == 1)
            else
              @read = false
              @write = false
              @execute = false
              perm_chars.each do |perm|
                case perm
                when 'r'
                  @read = true
                when 'w'
                  @write = true
                when 'x'
                  @execute = true
                when '-'
                else
                  raise AppSpecValidationException, "unrecognized permission character #{perm} in #{ace}"
                end
              end
            end
          end

          #format [default:][user|group|mask|other]:[name]:(r|-)(w|-)(x|-)
          def get_ace
            result = "";
            if @default
              result = "default:"
            end
            result = result + type + ":" + name + ":"
            if (@read)
              result = result + "r"
            else
              result = result + "-"
            end
            if (@write)
              result = result + "w"
            else
              result = result + "-"
            end
            if (@execute)
              result = result + "x"
            else
              result = result + "-"
            end
            result
          end

          def to_bits(num, min_size)
            bits = Array.new(min_size, 0)
            num_bits = num.to_s(2).split("")
            diff = [0, min_size - num_bits.length].max
            num_bits.map.with_index {|n,i| bits[i+diff] = n.to_i}
            bits
          end
        end

      end
    end
  end
end