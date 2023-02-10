# frozen_string_literal: true

require 'test_helper'
require 'stringio'
require 'fileutils'

class NestedAppspecHandlerTest < InstanceAgentTestCase
  class WindowsGlobber
    def self.glob(path)
      Dir.glob(path.gsub('\\', '/'))
    end
  end

  include InstanceAgent::Plugins::CodeDeployPlugin

  context "top level appspec" do
    setup do
      @deployment_root_dir = File.join('/tmp', 'nested-appspec-handler-test')
      @deployment_archive_dir = File.join(@deployment_root_dir, 'deployment-archive')
      FileUtils.rm_rf(@deployment_root_dir)
      Dir.mkdir(@deployment_root_dir)
      Dir.mkdir(@deployment_archive_dir)
      @appspec_path = File.join(@deployment_archive_dir, 'appspec.yml')
      @other_file_path = File.join(@deployment_archive_dir, 'otherfile.txt')
      FileUtils.touch(@appspec_path)
      FileUtils.touch(@other_file_path)
      @handler = NestedAppspecHandler.new(@deployment_root_dir, Dir)
    end

    should "do nothing" do
      @handler.handle
      assert(File.exist?(@appspec_path))
      assert(File.exist?(@other_file_path))
    end

    teardown do
      FileUtils.rm_rf(@deployment_root_dir)
    end
  end

  context "nested appspec" do
    setup do
      @deployment_root_dir = File.join('/tmp', 'nested-appspec-handler-test')
      @deployment_archive_dir = File.join(@deployment_root_dir, 'deployment-archive')
      @nested_dir = File.join(@deployment_archive_dir, 'nested')
      FileUtils.rm_rf(@deployment_root_dir)
      Dir.mkdir(@deployment_root_dir)
      Dir.mkdir(@deployment_archive_dir)
      Dir.mkdir(@nested_dir)
      @appspec_path = File.join(@nested_dir, 'appspec.yml')
      @toplevel_file_path = File.join(@deployment_archive_dir, 'toplevel_file.txt')
      @nested_file_path = File.join(@nested_dir, 'nested_file.txt')
      FileUtils.touch(@appspec_path)
      FileUtils.touch(@toplevel_file_path)
      FileUtils.touch(@nested_file_path)
      @handler = NestedAppspecHandler.new(@deployment_root_dir, Dir)
    end

    should "move nested contents to top level" do
      @handler.handle
      assert(File.exist?(File.join(@deployment_archive_dir, 'appspec.yml')), "Appspec does not exist at top level")
      assert(File.exist?(File.join(@deployment_archive_dir, 'nested_file.txt')), "Nested file does not exist at top level")
      assert(!File.exist?(File.join(@deployment_archive_dir, 'toplevel_file.txt')), "Original top-level file still exists")
    end

    teardown do
      FileUtils.rm_rf(@deployment_root_dir)
    end
  end

  context "Windows root dir with backslashes" do
    should "do nothing" do
      @deployment_root_dir = File.join('/tmp', 'nested-appspec-handler-test')
      @deployment_archive_dir = File.join(@deployment_root_dir, 'deployment-archive')
      FileUtils.rm_rf(@deployment_root_dir)
      Dir.mkdir(@deployment_root_dir)
      Dir.mkdir(@deployment_archive_dir)
      @appspec_path = File.join(@deployment_archive_dir, 'appspec.yml')
      @other_file_path = File.join(@deployment_archive_dir, 'otherfile.txt')
      FileUtils.touch(@appspec_path)
      FileUtils.touch(@other_file_path)
      @handler = NestedAppspecHandler.new('/tmp\\nested-appspec-handler-test', WindowsGlobber)

      @handler.handle

      assert(File.exist?(@appspec_path))
      assert(File.exist?(@other_file_path))

      FileUtils.rm_rf(@deployment_root_dir)
    end
  end
end