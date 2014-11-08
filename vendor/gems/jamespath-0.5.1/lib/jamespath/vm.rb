module Jamespath
  # The virtual machine that interprets compiled expressions and searches for
  # objects. The VM implements a handful of instructions that can be used to
  # navigate through an object structure.
  #
  # # VM Overview
  #
  # The VM iterates over the instructions attempting to navigate through
  # the given object. As instructions are evaluated, the "object" is tracked
  # and replaced as each selection is made. The result of a search is the
  # object value after evaluating all instructions.
  #
  # The VM understands "hash-like" and "array-like" objects. "Array-like"
  # objects are defined as any object that subclasses Array. "Hash-like"
  # objects are defined as either Hash or Struct objects.
  #
  # ## Instruction list
  #
  # ### `:get_key <key>`
  #
  # Gets a "key" from the hash-like object on the stack. If the object is
  # not hash-like, this instruction sets the object value to nil.
  #
  # ### `:get_idx <idx>`
  #
  # Gets an object at index "idx" from the array-like object on the stack.
  # If the object is not array-like, this instruction sets the object value
  # to nil. "idx" must be a number, but can be negative. Negative values
  # index from the end of the array, where -1 is the last value.
  #
  # ### `:get_key_all`
  #
  # Gets all values from the hash-like object value. If the object is not
  # hash-like, this instruction sets the object value to nil.
  #
  # ### `:get_idx_all`
  #
  # Gets all items from an array-like object. If the object is hash-like,
  # the object is set to the keys of the hash-like structure. If the object
  # is not array-like or hash-like, this instruction sets the object value
  # to nil.
  #
  # ### `:flatten_list`
  #
  # Flattens a list of subarrays into a single array. If the object is not
  # array-like, this instruction sets the object value to an empty array.
  #
  # ### `:ret_if_match`
  #
  # Breaks from parsing instructions if the object value is non-nil. If the
  # object is nil, this instruction should reset the object value to the
  # original object that was being searched.
  #
  class VM
    # @api private
    class ArrayGroup < Array
      def initialize(arr) replace(arr) end
    end

    # @return [Array(Symbol, Object)] the instructions the VM executes.
    attr_reader :instructions

    # Creates a virtual machine that can evaluate a set of instructions.
    # Use the {Parser} to turn an expression into a set of instructions.
    #
    # @param instructions [Array(Symbol, Object)] a list of instructions to
    #   execute.
    # @see Parser#parse
    # @example VM for expression "foo.bar[-1]"
    #   vm = VM.new [
    #    [:get_key, 'foo'],
    #    [:get_key, 'bar'],
    #    [:get_idx, -1]
    #   ]
    #   vm.search(foo: {bar: [1, 2, 3]}) #=> 3
    def initialize(instructions)
      @instructions = instructions
    end

    # Searches for the compile expression against the object passed in.
    #
    # @param object_to_search [Object] the object to search for results.
    # @return (see Jamespath.search)
    def search(object_to_search)
      object = object_to_search
      @instructions.each do |instruction|
        if instruction.first == :ret_if_match
          if object
            break # short-circuit or expression
          else
            object = object_to_search  # reset search
          end
        else
          object = send(instruction[0], object, instruction[1])
        end
      end

      object
    end

    protected

    def get_key(object, key)
      if struct?(object)
        object[key]
      elsif ArrayGroup === object
        object = object.map {|o| get_key(o, key) }.compact
        object.length > 0 ? ArrayGroup.new(object) : ArrayGroup.new([])
      end
    end

    def get_idx(object, idx)
      if ArrayGroup === object
        object = object.map {|o| get_idx(o, idx) }.compact
        object.length > 0 ? ArrayGroup.new(object) : nil
      elsif array?(object)
        object[idx]
      end
    end

    def get_key_all(object, *)
      object.respond_to?(:values) ? ArrayGroup.new(object.values) : nil
    end

    def get_idx_all(object, *)
      if array?(object)
        new_object = object.map do |o|
          Array === o ? ArrayGroup.new(o) : o
        end
        ArrayGroup.new(new_object)
      elsif object.respond_to?(:keys)
        ArrayGroup.new(object.keys)
      elsif object.respond_to?(:members)
        ArrayGroup.new(object.members.map(&:to_s))
      end
    end

    def flatten_list(object, *)
      if array?(object)
        new_object = []
        object.each {|o| array?(o) ? (new_object += o) : new_object << o }
        ArrayGroup.new(new_object)
      else
        []
      end
    end

    private

    def struct?(object)
      Hash === object || Struct === object
    end

    def array?(object)
      Array === object
    end
  end
end
