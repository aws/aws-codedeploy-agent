# encoding: UTF-8
require 'socket'
require 'etc'

unless defined?(ProcessManager)
  $: << File.expand_path(File.dirname(__FILE__) + '/lib')
  require_relative 'core_ext'
  require 'process_manager/config'
  require 'process_manager/log'
  require 'process_manager/master'
  require 'process_manager/child'

  module ProcessManager
    VERSION = '0.0.13'

    def self.process_running?(pid)
      begin
        Process.kill(0, Integer(pid))
        return true
      rescue Errno::EPERM # changed uid
        return false
      rescue Errno::ESRCH # deceased or zombied
        return false
      rescue
        puts "ERROR: couldn't check the status of process #{pid}"
        return false
      end
    end

    def self.set_program_name(name)
      $PROGRAM_NAME = "#{ProcessManager::Config.config[:program_name]}: #{name}"
    end

    def self.on_error(&block)
      @@_error_callbacks ||= []
      @@_error_callbacks << block
      nil
    end

    def self.on_error_callbacks
      @@_error_callbacks ||= []
    end

    def self.reset_on_error_callbacks
      @@_error_callbacks = []
    end

  end
end
