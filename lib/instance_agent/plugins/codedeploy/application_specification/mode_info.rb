module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper Class for storing mode of a file
        class ModeInfo

          attr_reader :mode
          attr_reader :world, :world_readable, :world_writable, :world_executable
          attr_reader :group, :group_readable, :group_writable, :group_executable
          attr_reader :owner, :owner_readable, :owner_writable, :owner_executable
          attr_reader :setuid, :setgid, :sticky
          def initialize(mode)
            mode = mode.to_s
            while mode.length < 3 do
              mode = "0" + mode;
            end
            if mode.length > 4
              raise AppSpecValidationException, "permission mode length incorrect: #{mode}"
            end
            mode.each_char do |char|
              if (char.ord < '0'.ord) || (char.ord > '7'.ord)
                raise AppSpecValidationException, "invalid character #{char} in permission mode #{mode}"
              end
            end
            @mode = mode
            mode_array = mode.reverse.chars.entries

            @world = mode_array[0]
            world_bits = to_bits(@world.to_i, 3)
            @world_readable = (world_bits[0] == 1)
            @world_writable = (world_bits[1] == 1)
            @world_executable = (world_bits[2] == 1)

            @group = mode_array[1]
            group_bits = to_bits(@group.to_i, 3)
            @group_readable = (group_bits[0] == 1)
            @group_writable = (group_bits[1] == 1)
            @group_executable = (group_bits[2] == 1)

            @owner = mode_array[2]
            owner_bits = to_bits(@owner.to_i, 3)
            @owner_readable = (owner_bits[0] == 1)
            @owner_writable = (owner_bits[1] == 1)
            @owner_executable = (owner_bits[2] == 1)

            special = (mode_array.length > 3) ? mode_array[3]: '0'
            special_bits = to_bits(special.to_i, 3)
            @setuid = (special_bits[0] == 1)
            @setgid = (special_bits[1] == 1)
            @sticky = (special_bits[2] == 1)
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