# encoding: UTF-8
$:.unshift "lib"
Gem.use_paths(nil, Gem.path << "vendor")

require 'coveralls'
Coveralls.wear!
require 'thread'
require 'rubygems'
require "bundler"
Bundler.require(:default, :test)

# test framework
require 'test/unit'
require 'active_support/testing/assertions'
require 'shoulda'
require 'mocha/setup'
require 'base64'

# require local test helpers. If you need a helper write,
# keep this pattern or you'll be punished hard
require 'instance_agent_helper'
require 'instance_agent/string_utils'