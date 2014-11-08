# encoding: UTF-8
require 'simple_pid'
require 'fileutils'
require 'blank'

module ProcessManager
  module Daemon
    class Master

      attr_accessor :children

      def initialize
        @children = {}
        ProcessManager.set_program_name(description)
        ensure_validate_configuration
        dropped_privileges = drop_privileges
        ProcessManager::Log.init(log_file)
        if dropped_privileges
          ProcessManager::Log.info("Dropped privileges to group: #{Etc.getgrgid(Process.gid).name} (gid = #{Process.gid})")
          ProcessManager::Log.info("Dropped privileges to user: #{Etc.getpwuid(Process.uid).name} (uid = #{Process.uid})")
        end
        after_initialize
      end

      # please override
      def after_initialize
        # hook
      end

      # please override
      def validate_ssl_config
      end

      def self.start
        pid = fork do
          new.start
        end
        Process.detach pid
      end

      def self.stop
        new.stop
      end

      def self.restart
        stop
        sleep 1
        start
      end

      def self.status
        if pid = find_pid
          if ProcessManager::process_running?(pid)
            pid
          else
            clean_stale_pid
            nil
          end
        else
          # does not run
          nil
        end
      end

      def self.pid_file
        File.join(ProcessManager::Config.config[:pid_dir], "#{ProcessManager::Config.config[:program_name]}.#{self.pid_description}.pid")
      end

      def self.pid_lock_file
        File.join(ProcessManager::Config.config[:pid_dir], "#{ProcessManager::Config.config[:program_name]}.pid.lock")
      end

      def pid_lock_file
        self.class.pid_lock_file
      end

      def pid_file
        self.class.pid_file
      end

      # please override
      def self.pid_description
        "ProcessManager"
      end

      def pid_description
        self.class.pid_description
      end

      def self.log_file
        File.join(ProcessManager::Config.config[:log_dir], "#{ProcessManager::Config.config[:program_name]}.#{pid_description}.log")
      end

      def log_file
        self.class.log_file
      end

      def drop_privileges

        runas_user = ProcessManager::Config.config[:user]
        return false if runas_user.blank?

        if runas_user == Etc.getpwuid(Process.uid).name
          return false
        elsif Process.uid != 0
          raise "Can't drop privileges as unprivileged user. Please run this command as a privileged user."
        end

        if runas_user.present?
          uid = Etc.getpwnam(runas_user).uid
          if (group = ProcessManager::Config.config[:group]) && group.present?
            gid = Etc.getgrnam(group).gid
          else
            gid = Etc.getpwuid(uid).gid
          end
          Process.initgroups(runas_user, gid)
          Process::GID.change_privilege(gid)
          Process::UID.change_privilege(uid)
          true
        end
        false
      rescue Exception => e
        $stderr.puts "Failed to drop privileges: #{e.class} - #{e.message} - #{e.backtrace.join("\n")}"
        exit 1
      end

      def start
        handle_pid_file
        validate_ssl_config
        trap_signals

        spawn_children
        puts "Started #{description} with #{ProcessManager::Config.config[:children]} children"
        ProcessManager::Log.info("Started #{description} with #{ProcessManager::Config.config[:children]} children")

        loop do
          # master does nothing apart from replacing dead children
          # and forwarding signals
          sleep 1
        end
      end

      def stop
        if (pid = self.class.find_pid)
          puts "Stopping #{description(pid)}"
          ProcessManager::Log.info("Stopping #{description(pid)}")
          begin
            Process.kill('TERM', pid)
          rescue Errno::ESRCH
          end
        else
          puts "Nothing running that could be stopped"
        end
      end

      def handle_pid_file       
        @file_lock ||= File.open(pid_lock_file, File::RDWR|File::CREAT, 0644)
        lock_aquired = @file_lock.flock(File::LOCK_EX|File::LOCK_NB)
        
        if lock_aquired == false
          ProcessManager::Log.info("Could not aquire lock on #{pid_lock_file} - aborting start!")
          self.class.abort
        
        elsif File.exists?(pid_file)
          pid = self.class.find_pid
          if ProcessManager.process_running?(pid)
            puts "Pidfile #{pid_file} exists and process #{pid} is running - aborting start!"
            ProcessManager::Log.info("Pidfile #{pid_file} exists and process #{pid} is running - aborting start!")
            @file_lock.close
            self.class.abort
          else
            self.class.clean_stale_pid
          end
        end
        ::SimplePid.drop(pid_file)
      end

      def spawn_children
        ProcessManager::Config.config[:children].times do |i|
          spawn_child(i)
          sleep ProcessManager::Config.config[:wait_between_spawning_children].to_i
        end
      end

      # spawn a new child and pass down out PID so that it can check if we are alive
      def spawn_child(index)
        master_pid = $$ # need to store in order to pass down to child
        child_pid = fork do
          child_class.new(index, master_pid).start
        end
        children[index] = child_pid
        ProcessManager::Log.info "#{description}: Spawned child #{index + 1}/#{ProcessManager::Config.config[:children]}"
      end

      def trap_signals
        # The QUIT & INT signals triggers a graceful shutdown.
        # The master shuts down immediately and forwards the signal to each child
        [:INT, :QUIT, :TERM].each do |sig|
          trap(sig) do
            ProcessManager::Log.info "#{description}: Received #{sig} - stopping children and shutting down"
            kill_children(sig)
            cleanup_and_exit
          end
        end

        trap(:CHLD) do
          handle_chld
        end
      end

      def cleanup_and_exit
        SimplePid.cleanup!(pid_file)
        @file_lock.close
        exit
      end

      def handle_chld
        if child = reap_child
          ProcessManager::Log.info "#{description}: Received CHLD - cleaning dead child process"
          cleanup_dead_child(child)
        else
          ProcessManager::Log.debug "#{description}: Received CHLD - ignoring as it looks like a child of a child"
        end
      end

      def reap_child
        dead_child = nil
        begin
          dead_child = Process.wait
        rescue Errno::ECHILD
        end
        dead_child
      end

      def cleanup_dead_child(dead_child)
        ProcessManager::Log.info "#{description}: been told to replace child #{dead_child.inspect}"
        # delete given child
        if index = children.key(dead_child)
          children.delete(index)
        end

        # check all other children
        children.each do |child_index, child_pid|
          begin
            dead_child = Process.waitpid(child_pid, Process::WNOHANG)
            if index = children.key(dead_child)
              children.delete(index)
            end
          rescue Errno::ECHILD
          end
        end

        replace_terminated_children
      end

      # make sure we have again as many child we need
      def replace_terminated_children
        missing_children = ProcessManager::Config.config[:children] - children.values.size
        if missing_children > 0
          ProcessManager::Log.info "#{description}: not enough child processes running - missing at least #{missing_children} - respawning"
          0.upto(ProcessManager::Config.config[:children] - 1).each do |i|
            if children.has_key?(i)
              ProcessManager::Log.debug "#{description}: child #{i+1}/#{ProcessManager::Config.config[:children]} is still there"
            else
              spawn_child(i)
            end
          end
        else
          ProcessManager::Log.debug "#{description}: no need to replace child processes"
        end
      end

      def kill_children(sig)
        children.each do |index, child_pid|
          begin
            Process.kill(sig, child_pid)
          rescue Errno::ESRCH
          end
        end
      end

      def ensure_validate_configuration
        if (errors = ProcessManager::Config.validate_config)
          errors.each{|error| puts error}
        end
        cleanup_and_exit unless errors.empty?
      end

      def self.abort
        Kernel.abort
      end

      def self.clean_stale_pid
        puts "Pidfile #{pid_file} present but no matching process running - cleaning up"
        ::FileUtils.rm(pid_file)
      end

      def self.find_pid
        File.read(pid_file).chomp.to_i rescue nil
      end

      def description(pid = $$)
        self.class.description(pid)
      end

      def child_class
        self.class.child_class
      end

      # please override
      def self.description(pid = $$)
        "master #{pid}"
      end

      # please override
      def self.child_class
        ::ProcessManager::Daemon::Child
      end

    end
  end
end
