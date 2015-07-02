module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification
        #Helper Class for storing an acl
        class AclInfo

          attr_reader :aces, :additional
          def initialize(acl)
            @aces = []
            @additional = []
            acl.each do |ace|
              @aces << AceInfo.new(ace)
            end
          end

          #format [default:][user|group|mask|other]:[name]:(r|-)(w|-)(x|-) (or nil if none present)
          def get_default_ace
            @aces.each do |ace|
              if ace.default
                return ace.get_ace
              end
            end
            @additional.each do |ace|
              if ace.default
                return ace.get_ace
              end
            end
            nil
          end

          #format array of aces with format: [default:][user|group|mask|other]:[name]:(r|-)(w|-)(x|-)
          def get_acl
            aces = []
            @aces.each do |ace|
              aces << ace.get_ace
            end
            @additional.each do |ace|
              aces << ace.get_ace
            end
            aces
          end

          def add_ace(ace)
            additional << AceInfo.new(ace, true)
          end

          def clear_additional()
            additional = []
          end

          def has_base_named?
            @aces.each do |ace|
              if !ace.default && !ace.name.eql?("")
                return true
              end
            end
            @additional.each do |ace|
              if !ace.default && !ace.name.eql?("")
                return true
              end
            end
            false
          end

          def has_base_mask?
            @aces.each do |ace|
              if !ace.default && ace.type.eql?("mask")
                return true
              end
            end
            @additional.each do |ace|
              if !ace.default && ace.type.eql?("mask")
                return true
              end
            end
            false
          end

          def has_default?
            !get_default_ace.nil?
          end

          def has_default_user?
            @aces.each do |ace|
              if ace.default && ace.type.eql?("user") && ace.name.eql?("")
                return true
              end
            end
            @additional.each do |ace|
              if ace.default && ace.type.eql?("user") && ace.name.eql?("")
                return true
              end
            end
            false
          end

          def has_default_group?
            !get_default_group_ace.nil?
          end

          #format [default:][user|group|mask|other]:[name]:(r|-)(w|-)(x|-) (or nil if not present)
          def get_default_group_ace
            @aces.each do |ace|
              if ace.default && ace.type.eql?("group") && ace.name.eql?("")
                return ace.get_ace
              end
            end
            @additional.each do |ace|
              if ace.default && ace.type.eql?("group") && ace.name.eql?("")
                return ace.get_ace
              end
            end
            nil
          end

          def has_default_other?
            @aces.each do |ace|
              if ace.default && ace.type.eql?("other")
                return true
              end
            end
            @additional.each do |ace|
              if ace.default && ace.type.eql?("other")
                return true
              end
            end
            false
          end

          def has_default_named?
            @aces.each do |ace|
              if ace.default && !ace.name.eql?("")
                return true
              end
            end
            @additional.each do |ace|
              if ace.default && !ace.name.eql?("")
                return true
              end
            end
            false
          end

          def has_default_mask?
            @aces.each do |ace|
              if ace.default && ace.type.eql?("mask")
                return true
              end
            end
            @additional.each do |ace|
              if ace.default && ace.type.eql?("mask")
                return true
              end
            end
            false
          end
        end

      end
    end
  end
end