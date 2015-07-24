# encoding: UTF-8

Gem.use_paths(nil, Gem.path << "/opt/codedeploy-agent/vendor")

require 'core_ext'

require 'rubygems'

require 'process_manager'

unless defined?(InstanceAgent)
  require 'instance_agent/config'
  require 'instance_agent/log'
  require 'instance_agent/platform'
  require 'instance_agent/platform/linux_util'
  require 'instance_agent/agent/plugin'
  require 'instance_agent/agent/base'
  require 'instance_agent/runner/master'
  require 'instance_agent/runner/child'
end

module InstanceAgent

  module Runner
  end

  module Agent
  end
end
