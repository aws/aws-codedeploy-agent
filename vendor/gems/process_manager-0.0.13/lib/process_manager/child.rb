# encoding: UTF-8
module ProcessManager
  module Daemon
    class Child

      attr_accessor :times_run, :master_pid, :index

      # each child gets the PID of the master
      # the child checks regularly if the master is alive and terminates itself if not
      def initialize(index, master_pid)
        @index = index
        @master_pid = master_pid
        @times_run = 0

        ProcessManager.set_program_name(description)
      end

      def start
        trap_signals
        prepare_run_with_error_handling

        loop do
          if run_limit_met?
            ProcessManager::Log.info "#{description}: ran #{times_run} - shutting down"
            exit
          elsif should_stop?
            ProcessManager::Log.info "#{description}: shutting down"
            exit
          else # the main loop
            if master_alive?

              # the actual main running method
              run_with_error_handling
              increase_run_counter
            else
              ProcessManager::Log.info "#{description}: Master #{master_pid} not alive - shutting down"
              exit
            end
          end

          sleep ProcessManager::Config.config[:wait_between_runs].to_i
        end
      end

      def increase_run_counter
        @times_run += 1
      end

      def run_limit_met?
        limit = ProcessManager::Config.config[:max_runs_per_worker].to_i
        return false if limit == 0
        times_run >= limit
      end

      def with_error_handling
        yield
      rescue Exception => e
        ProcessManager::Log.error "#{description}: error during start: #{e.class} - #{e} - #{e.backtrace.join("\n")}"
        exit 1
      end

      def prepare_run_with_error_handling
        with_error_handling do
          prepare_run
        end
      end

      def run_with_error_handling
        with_error_handling do
          run
        end
      end

      # please override
      def run
        ProcessManager::Log.info "Hello from #{description}"
      end

      # please override
      def prepare_run
      end

      def stop
        @should_stop = true
        ProcessManager.set_program_name("#{description} - shutting down")
      end

      def should_stop?
        @should_stop
      end

      def master_alive?
        ProcessManager.process_running?(master_pid)
      end

      # please override
      def description
        "child #{position} (#{$$}) of master #{master_pid}"
      end

      def position
        "#{index + 1}/#{ProcessManager::Config.config[:children]}"
      end

      def trap_signals
        [:INT, :QUIT, :TERM].each do |sig|
          trap(sig) do
            ProcessManager::Log.info "#{description}: Received #{sig} - setting internal shutting down flag and possibly finishing last run"
            stop
          end
        end

        # make sure we do not handle children like the master process
        trap(:CHLD, 'DEFAULT')
      end

    end
  end
end
