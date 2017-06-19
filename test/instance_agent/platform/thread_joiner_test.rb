require 'test_helper'

class ThreadJoinerTest < InstanceAgentTestCase
  context 'ThreadJoiner' do
    setup do
      @timeout_sec = 30
      @thread1 = mock('thread1')
      @thread2 = mock('thread2')
      @start_time = Time.now
      Time.stubs(:now).returns(start_time)
      @joiner = InstanceAgent::ThreadJoiner.new(@timeout_sec)
    end

    context 'with time left' do
      setup do
	@thread1.expects(:join).with(@timeout_sec).returns(1)
	@thread2.expects(:join).with(@timeout_sec - 13).returns(1)
      end

      should 'join threads with proper timeout' do
        @joiner.joinOrFail(@thread1)
        Time.stubs(:now).returns(@start_time + 13)
        @joiner.joinOrFail(@thread2)
      end
    end

    context 'with no time left' do
      setup do
        @thread1.expects(:join).with(0)
        @thread2.expects(:join).with(0)
      end

      should 'join threads for zero seconds' do
        Time.stubs(:now).returns(@start_time + @timeout_sec)
        @joiner.joinOrFail(@thread1)
        Time.stubs(:now).returns(@start_time + @timeout_sec + 1)
        @joiner.joinOrFail(@thread2)
      end
    end

    context 'when a block is provided' do
      context 'and thread does not complete' do
        setup do
          @thread1.expects(:join).returns(nil)
        end

        should 'call block' do
          called = false
          @joiner.joinOrFail(@thread1) do
            called = true
          end

          assert_true called
        end

        should 'pass thread to block' do
          thread = nil
          @joiner.joinOrFail(@thread1) do | th |
            thread = th
          end

          assert_equal @thread1, thread
        end

        should 'propagate exception back from block' do
          assert_raise RuntimeError do
            @joiner.joinOrFail(@thread1) do
              raise 'thread did not complete'
            end
          end
        end
      end

      context 'and thread completes' do
        setup do
          @thread1.expects(:join).returns(1)
        end

        should 'not call block' do
          called = false
          @joiner.joinOrFail(@thread1) do
            called = true
          end

          assert_false called
        end
      end
    end
  end
end
