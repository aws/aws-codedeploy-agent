# frozen_string_literal: true
require 'fileutils'

module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      class NestedAppspecHandler
        def initialize(deployment_root_dir, globber)
          @deployment_root_dir = deployment_root_dir
          @globber = globber
        end

        def handle
          log(:debug, "Checking deployment archive rooted at #{archive_root} for appspec...")
          # if the root of the archive doesn't contain an appspec, and there is exactly one
          # directory with appspec, move top level to said directory
          archive_root_appspec = @globber.glob(File.join(archive_root, 'appspec.*'))
          archive_nested_appspec = @globber.glob(File.join(archive_root, '*', 'appspec.*'))
          total_appspecs = archive_root_appspec.size + archive_nested_appspec.size

          if total_appspecs == 0
            log_and_raise_not_found
          elsif total_appspecs > 1
            log(:warn, "There are multiple appspec files in the bundle")
          end

          if archive_root_appspec.size == 0 && archive_nested_appspec.size == 1
            strip_leading_directory
          end

          #once the nested directory is handled there should be only one appspec file in the deployment-archive
          if @globber.glob(File.join(archive_root, 'appspec.*')).size < 1
            log_and_raise_not_found
          end
        end

        private
        def archive_root
          File.join(@deployment_root_dir, 'deployment-archive')
        end

        private
        def strip_leading_directory
          log(:info, "Stripping leading directory from archive bundle contents.")
          # Move the unpacked files to a temporary location
          tmp_dst = File.join(@deployment_root_dir, 'deployment-archive-temp')
          FileUtils.rm_rf(tmp_dst)
          FileUtils.mv(archive_root, tmp_dst)

          # Move the top level directory to the intended location
          nested_archive_root = File.dirname(@globber.glob(File.join(tmp_dst, '*', 'appspec.*'))[0])
          log(:debug, "Actual archive root at #{nested_archive_root}. Moving to #{archive_root}")
          FileUtils.mv(nested_archive_root, archive_root)
          remove_deployment_archive_temp(tmp_dst)
          log(:debug, Dir.entries(archive_root).join("; "))
        end

        def remove_deployment_archive_temp(tmp_dst)
          tmp_dst_files = Dir.entries(tmp_dst).to_set.subtract(['.', '..']).to_a.sort
          with_extra_message = tmp_dst_files[0,10].append("...#{tmp_dst_files.size - 10} additional files")
          warn_about_these = tmp_dst_files
          if with_extra_message.size <= tmp_dst_files.size # if <= 10 elements, we would only have added an element and not removed any
            warn_about_these = with_extra_message
          end

          if !warn_about_these.empty?
            log(:warn, "The following files are outside the directory containing appspec and will be removed: #{warn_about_these.join(';')}")
          end

          FileUtils.rm_rf(tmp_dst)
        end

        def log_and_raise_not_found
          msg = "appspec file is not found."
          log(:error, msg)
          raise msg
        end

        def log(severity, message)
          raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
          InstanceAgent::Log.send(severity.to_sym, "#{description}: #{message}")
        end

        def description
          self.class.to_s
        end
      end
    end
  end
end

