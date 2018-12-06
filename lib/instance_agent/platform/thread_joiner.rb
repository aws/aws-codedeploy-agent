module InstanceAgent
  class ThreadJoiner
    def initialize(timeout_sec)
      @timeout_epoch = Time.now.to_i + timeout_sec
    end

    def joinOrFail(thread, &block)
      if !thread.join([@timeout_epoch - Time.now.to_i, 0].max)
        yield(thread) if block_given?
      end
    end
  end
end
