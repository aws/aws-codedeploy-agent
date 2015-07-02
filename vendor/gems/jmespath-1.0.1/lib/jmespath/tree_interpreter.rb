module JMESPath
  # @api private
  class TreeInterpreter

    def visit(node, data)
      dispatch(node, data)
    end

    # @api private
    def method_missing(method_name, *args)
      if matches = method_name.to_s.match(/^function_(.*)/)
        raise Errors::UnknownFunctionError, "unknown function #{matches[1]}()"
      else
        super
      end
    end

    private

    def dispatch(node, value)
      case node[:type]

      when :field
        # hash_like?
        key = node[:key]
        case value
        when Hash then value.key?(key) ? value[key] : value[key.to_sym]
        when Struct then value.respond_to?(key) ? value[key] : nil
        else nil
        end

      when :subexpression
        dispatch(node[:children][1], dispatch(node[:children][0], value))

      when :index
        if Array === value
          value[node[:index]]
        else
          nil
        end

      when :projection
        # Interprets a projection node, passing the values of the left
        # child through the values of the right child and aggregating
        # the non-null results into the return value.
        left = dispatch(node[:children][0], value)
        if node[:from] == :object && hash_like?(left)
          projection(left.values, node)
        elsif node[:from] == :object && left == []
          projection(left, node)
        elsif node[:from] == :array && Array === left
          projection(left, node)
        else
          nil
        end

      when :flatten
        value = dispatch(node[:children][0], value)
        if Array === value
          value.inject([]) do |values, v|
            values + (Array === v ? v : [v])
          end
        else
          nil
        end

      when :literal
        node[:value]

      when :current
        value

      when :or
        result = dispatch(node[:children][0], value)
        if result.nil? or result.empty?
          dispatch(node[:children][1], value)
        else
          result
        end

      when :pipe
        dispatch(node[:children][1], dispatch(node[:children][0], value))

      when :multi_select_list
        if value.nil?
          value
        else
          node[:children].map { |n| dispatch(n, value) }
        end

      when :multi_select_hash
        if value.nil?
          nil
        else
          node[:children].each.with_object({}) do |child, hash|
            hash[child[:key]] = dispatch(child[:children][0], value)
          end
        end


      when :comparator
        left = dispatch(node[:children][0], value)
        right = dispatch(node[:children][1], value)
        case node[:relation]
        when '==' then compare_values(left, right)
        when '!=' then !compare_values(left, right)
        when '>' then is_int(left) && is_int(right) && left > right
        when '>=' then is_int(left) && is_int(right) && left >= right
        when '<' then is_int(left) && is_int(right) && left < right
        when '<=' then is_int(left) && is_int(right) && left <= right
        end

      when :condition
        true == dispatch(node[:children][0], value) ?
          dispatch(node[:children][1], value) :
          nil

      when :function
        args = node[:children].map { |child| dispatch(child, value) }
        send("function_#{node[:fn]}", *args)

      when :slice
        function_slice(value, *node[:args])

      when :expression
        ExprNode.new(self, node[:children][0])

      else
        raise NotImplementedError
      end
    end

    def hash_like?(value)
      Hash === value || Struct === value
    end

    def projection(values, node)
      values.inject([]) do |list, v|
        list << dispatch(node[:children][1], v)
      end.compact
    end

    def function_abs(*args)
      if args.count == 1
        value = args.first
      else
        raise Errors::InvalidArityError, "function abs() expects one argument"
      end
      if Numeric === value
        value.abs
      else
        raise Errors::InvalidTypeError, "function abs() expects a number"
      end
    end

    def function_avg(*args)
      if args.count == 1
        values = args.first
      else
        raise Errors::InvalidArityError, "function avg() expects one argument"
      end
      if Array === values
        values.inject(0) do |total,n|
          if Numeric === n
            total + n
          else
            raise Errors::InvalidTypeError, "function avg() expects numeric values"
          end
        end / values.size.to_f
      else
        raise Errors::InvalidTypeError, "function avg() expects a number"
      end
    end

    def function_ceil(*args)
      if args.count == 1
        value = args.first
      else
        raise Errors::InvalidArityError, "function ceil() expects one argument"
      end
      if Numeric === value
        value.ceil
      else
        raise Errors::InvalidTypeError, "function ceil() expects a numeric value"
      end
    end

    def function_contains(*args)
      if args.count == 2
        if String === args[0] || Array === args[0]
          args[0].include?(args[1])
        else
          raise Errors::InvalidTypeError, "contains expects 2nd arg to be a list"
        end
      else
        raise Errors::InvalidArityError, "function contains() expects 2 arguments"
      end
    end

    def function_floor(*args)
      if args.count == 1
        value = args.first
      else
        raise Errors::InvalidArityError, "function floor() expects one argument"
      end
      if Numeric === value
        value.floor
      else
        raise Errors::InvalidTypeError, "function floor() expects a numeric value"
      end
    end

    def function_length(*args)
      if args.count == 1
        value = args.first
      else
        raise Errors::InvalidArityError, "function length() expects one argument"
      end
      case value
      when Hash, Array, String then value.size
      else raise Errors::InvalidTypeError, "function length() expects string, array or object"
      end
    end

    def function_max(*args)
      if args.count == 1
        values = args.first
      else
        raise Errors::InvalidArityError, "function max() expects one argument"
      end
      if Array === values
        values.inject(values.first) do |max, v|
          if Numeric === v
            v > max ? v : max
          else
            raise Errors::InvalidTypeError, "function max() expects numeric values"
          end
        end
      else
        raise Errors::InvalidTypeError, "function max() expects an array"
      end
    end

    def function_min(*args)
      if args.count == 1
        values = args.first
      else
        raise Errors::InvalidArityError, "function min() expects one argument"
      end
      if Array === values
        values.inject(values.first) do |min, v|
          if Numeric === v
            v < min ? v : min
          else
            raise Errors::InvalidTypeError, "function min() expects numeric values"
          end
        end
      else
        raise Errors::InvalidTypeError, "function min() expects an array"
      end
    end

    def function_type(*args)
      if args.count == 1
        get_type(args.first)
      else
        raise Errors::InvalidArityError, "function type() expects one argument"
      end
    end

    def function_keys(*args)
      if args.count == 1
        value = args.first
        if hash_like?(value)
          case value
          when Hash then value.keys.map(&:to_s)
          when Struct then value.members.map(&:to_s)
          else raise NotImplementedError
          end
        else
          raise Errors::InvalidTypeError, "function keys() expects a hash"
        end
      else
        raise Errors::InvalidArityError, "function keys() expects one argument"
      end
    end

    def function_values(*args)
      if args.count == 1
        value = args.first
        if hash_like?(value)
          value.values
        elsif Array === value
          value
        else
          raise Errors::InvalidTypeError, "function values() expects an array or a hash"
        end
      else
        raise Errors::InvalidArityError, "function values() expects one argument"
      end
    end

    def function_join(*args)
      if args.count == 2
        glue = args[0]
        values = args[1]
        if !(String === glue)
          raise Errors::InvalidTypeError, "function join() expects the first argument to be a string"
        elsif Array === values && values.all? { |v| String === v }
          values.join(glue)
        else
          raise Errors::InvalidTypeError, "function join() expects values to be an array of strings"
        end
      else
        raise Errors::InvalidArityError, "function join() expects an array of strings"
      end
    end

    def function_to_string(*args)
      if args.count == 1
        value = args.first
        String === value ? value : MultiJson.dump(value)
      else
        raise Errors::InvalidArityError, "function to_string() expects one argument"
      end
    end

    def function_to_number(*args)
      if args.count == 1
        begin
          value = Float(args.first)
          Integer(value) === value ? value.to_i : value
        rescue
          nil
        end
      else
        raise Errors::InvalidArityError, "function to_number() expects one argument"
      end
    end

    def function_sum(*args)
      if args.count == 1 && Array === args.first
        args.first.inject(0) do |sum,n|
          if Numeric === n
            sum + n
          else
            raise Errors::InvalidTypeError, "function sum() expects values to be numeric"
          end
        end
      else
        raise Errors::InvalidArityError, "function sum() expects one argument"
      end
    end

    def function_not_null(*args)
      if args.count > 0
        args.find { |value| !value.nil? }
      else
        raise Errors::InvalidArityError, "function not_null() expects one or more arguments"
      end
    end

    def function_sort(*args)
      if args.count == 1
        value = args.first
        if Array === value
          value.sort do |a, b|
            a_type = get_type(a)
            b_type = get_type(b)
            if ['string', 'number'].include?(a_type) && a_type == b_type
              a <=> b
            else
              raise Errors::InvalidTypeError, "function sort() expects values to be an array of numbers or integers"
            end
          end
        else
          raise Errors::InvalidTypeError, "function sort() expects values to be an array of numbers or integers"
        end
      else
        raise Errors::InvalidArityError, "function sort() expects one argument"
      end
    end

    def function_sort_by(*args)
      if args.count == 2
        if get_type(args[0]) == 'array' && get_type(args[1]) == 'expression'
          values = args[0]
          expression = args[1]
          values.sort do |a,b|
            a_value = expression.interpreter.visit(expression.node, a)
            b_value = expression.interpreter.visit(expression.node, b)
            a_type = get_type(a_value)
            b_type = get_type(b_value)
            if ['string', 'number'].include?(a_type) && a_type == b_type
              a_value <=> b_value
            else
              raise Errors::InvalidTypeError, "function sort() expects values to be an array of numbers or integers"
            end
          end
        else
          raise Errors::InvalidTypeError, "function sort_by() expects an array and an expression"
        end
      else
        raise Errors::InvalidArityError, "function sort_by() expects two arguments"
      end
    end

    def function_max_by(*args)
      number_compare(:max, *args)
    end

    def function_min_by(*args)
      number_compare(:min, *args)
    end

    def number_compare(mode, *args)
      if args.count == 2
        if get_type(args[0]) == 'array' && get_type(args[1]) == 'expression'
          values = args[0]
          expression = args[1]
          args[0].send("#{mode}_by") do |entry|
            value = expression.interpreter.visit(expression.node, entry)
            if get_type(value) == 'number'
              value
            else
              raise Errors::InvalidTypeError, "function #{mode}_by() expects values to be an numbers"
            end
          end
        else
          raise Errors::InvalidTypeError, "function #{mode}_by() expects an array and an expression"
        end
      else
        raise Errors::InvalidArityError, "function #{mode}_by() expects two arguments"
      end
    end

    def function_slice(values, *args)
      if String === values || Array === values
        _slice(values, *args)
      else
        nil
      end
    end

    def _slice(values, start, stop, step)
      start, stop, step = _adjust_slice(values.size, start, stop, step)
      result = []
      if step > 0
        i = start
        while i < stop
          result << values[i]
          i += step
        end
      else
        i = start
        while i > stop
          result << values[i]
          i += step
        end
      end
      String === values ? result.join : result
    end

    def _adjust_slice(length, start, stop, step)
      if step.nil?
        step = 1
      elsif step == 0
        raise Errors::RuntimeError, 'slice step cannot be 0'
      end

      if start.nil?
        start = step < 0 ? length - 1 : 0
      else
        start = _adjust_endpoint(length, start, step)
      end

      if stop.nil?
        stop = step < 0 ? -1 : length
      else
        stop = _adjust_endpoint(length, stop, step)
      end

      [start, stop, step]
    end

    def _adjust_endpoint(length, endpoint, step)
      if endpoint < 0
        endpoint += length
        endpoint = 0 if endpoint < 0
        endpoint
      elsif endpoint >= length
        step < 0 ? length - 1 : length
      else
        endpoint
      end
    end

    def compare_values(a, b)
      if a == b
        true
      else
        false
      end
    end

    def is_int(value)
      Integer === value
    end

    def get_type(value)
      case
      when ExprNode === value then 'expression'
      when String === value then 'string'
      when hash_like?(value) then 'object'
      when Array === value then 'array'
      when [true, false].include?(value) then 'boolean'
      when value.nil? then 'null'
      when Numeric === value then 'number'
      end
    end

  end
end
