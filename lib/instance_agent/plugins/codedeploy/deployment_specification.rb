require 'openssl'
require 'instance_metadata'
require 'open-uri'

module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      class DeploymentSpecification
        DEFAULT_FILE_EXISTS_BEHAVIOR = 'DISALLOW'

        attr_accessor :deployment_id, :deployment_group_id, :deployment_group_name, :revision, :revision_source, :application_name, :deployment_type, :deployment_creator
        attr_accessor :bucket, :key, :bundle_type, :version, :etag
        attr_accessor :external_account, :repository, :commit_id, :anonymous, :external_auth_token
        attr_accessor :file_exists_behavior
        attr_accessor :local_location, :all_possible_lifecycle_events
        class << self
          attr_accessor :cert_store
        end

        def self.init_cert_store(ca_chain_path)
          @cert_store = OpenSSL::X509::Store.new
          begin
            @cert_store.add_file ca_chain_path
          rescue OpenSSL::X509::StoreError => e
            raise "Could not load certificate store '#{ca_chain_path}'.\nCaused by: #{e.inspect}"
          end
          return @cert_store
        end

        @cert_store = init_cert_store(File.expand_path('../../../../certs/host-agent-deployment-signer-ca-chain.pem', File.dirname(__FILE__)))

        def initialize(data)
          raise 'Deployment Spec has no DeploymentId' unless property_set?(data, "DeploymentId")
          raise 'Deployment Spec has no DeploymentGroupId' unless property_set?(data, "DeploymentGroupId")
          raise 'Deployment Spec has no DeploymentGroupName' unless property_set?(data, "DeploymentGroupName")
          raise 'Deployment Spec has no ApplicationName' unless property_set?(data, "ApplicationName")

          @application_name = data["ApplicationName"]
          @deployment_group_name = data["DeploymentGroupName"]

          if data["DeploymentId"].start_with?("arn:")
            @deployment_id = getDeploymentIdFromArn(data["DeploymentId"])
          else
            @deployment_id = data["DeploymentId"]
          end
          @deployment_group_id = data["DeploymentGroupId"]
          @deployment_creator = data["DeploymentCreator"] || "user"
          @deployment_type = data["DeploymentType"] || "IN_PLACE"

          raise 'Must specify a revison' unless data["Revision"]
          @revision_source = data["Revision"]["RevisionType"]
          raise 'Must specify a revision source' unless @revision_source

          @file_exists_behavior = DEFAULT_FILE_EXISTS_BEHAVIOR
          if property_set?(data, "AgentActionOverrides")
            agentActionsOverrides = data["AgentActionOverrides"]
            if property_set?(agentActionsOverrides,  "AgentOverrides")
              agentActionsOverridesMap = agentActionsOverrides["AgentOverrides"]
              if property_set?(agentActionsOverridesMap, "FileExistsBehavior")
                @file_exists_behavior = agentActionsOverridesMap["FileExistsBehavior"].upcase
              end
            end
          end

          if property_set?(data, 'AllPossibleLifecycleEvents')
            @all_possible_lifecycle_events = data['AllPossibleLifecycleEvents']
          end

          case @revision_source
          when 'S3'
            @revision = data["Revision"]["S3Revision"]
            raise 'S3Revision in Deployment Spec must specify Bucket, Key and BundleType' unless valid_s3_revision?(@revision)
            raise 'BundleType in S3Revision must be tar, tgz or zip' unless valid_bundle_type?(@revision)

            @bucket = @revision["Bucket"]
            @key = @revision["Key"]
            @bundle_type = @revision["BundleType"]
            @version = @revision["Version"]
            @etag = @revision["ETag"]
          when 'GitHub'
            @revision = data["Revision"]["GitHubRevision"]
            raise 'GitHubRevision in Deployment Spec must specify Account, Repository and CommitId' unless valid_github_revision?(revision)
            @external_account = revision["Account"]
            @repository = revision["Repository"]
            @commit_id = revision["CommitId"]
            @external_auth_token = data["GitHubAccessToken"]
            @anonymous = @external_auth_token.nil?
            @bundle_type = @revision["BundleType"]
          when 'Local File', 'Local Directory'
            @revision = data["Revision"]["LocalRevision"]
            raise 'LocalRevision in Deployment Spec must specify Location and BundleType' unless valid_local_revision?(revision)
            raise 'BundleType in LocalRevision must be tar, tgz, zip, or directory' unless valid_local_bundle_type?(@revision)

            @local_location = @revision["Location"]
            @bundle_type = @revision["BundleType"]
          else
            raise 'Exactly one of S3Revision, GitHubRevision, or LocalRevision must be specified'
          end
        end

        def self.parse(envelope)
          raise 'Provided deployment spec was nil' if envelope.nil?

          case envelope.format
          when "PKCS7/JSON"
            pkcs7 = OpenSSL::PKCS7.new(envelope.payload)
            pkcs7.verify([], @cert_store, nil, OpenSSL::PKCS7::NOVERIFY)
            # NOTE: the pkcs7.data field is only populated AFTER pkcs7.verify() is called!
            parse_deployment_spec_data(pkcs7.data)
          when "TEXT/JSON"
            raise "Unsupported DeploymentSpecification format: #{envelope.format}" unless AWS::CodeDeploy::Local::Deployer.running_as_developer_utility?
            # We only allow json unsigned messages from the local developer utility (codedeploy-local cli)
            # This is because the local cli cannot actually sign messages since it doens't have the private key
            # that the CodeDeploy service has.
            parse_deployment_spec_data(envelope.payload)
          else
            raise "Unsupported DeploymentSpecification format: #{envelope.format}"
          end
        end

        private
        def self.parse_deployment_spec_data(deployment_spec_data)
            deployment_spec = JSON.parse(deployment_spec_data)

            sanitized_spec = deployment_spec.clone
            sanitized_spec["GitHubAccessToken"] &&= "REDACTED"
            InstanceAgent::Log.debug("#{self.to_s}: Parse: #{sanitized_spec}")

            new(deployment_spec)
        end

        def property_set?(propertyHash, property)
          propertyHash.has_key?(property) && !propertyHash[property].nil? && !propertyHash[property].empty?
        end

        def valid_s3_revision?(revision)
          revision.nil? || %w(Bucket Key BundleType).all? { |k| revision.has_key?(k) }
        end

        def valid_github_revision?(revision)
          required_fields = %w(Account Repository CommitId)
          if !(revision.nil? || revision['Anonymous'].nil? || revision['Anonymous'])
            required_fields << 'OAuthToken'
          end
          revision.nil? || required_fields.all? { |k| revision.has_key?(k) }
        end

        def valid_local_revision?(revision)
          revision.nil? || %w(Location BundleType).all? { |k| revision.has_key?(k) }
        end

        private
        def valid_bundle_type?(revision)
          revision.nil? || %w(tar zip tgz).any? { |k| revision["BundleType"] == k }
        end

        def valid_local_bundle_type?(revision)
          revision.nil? || %w(tar zip tgz directory).any? { |k| revision["BundleType"] == k }
        end

        private
        def getDeploymentIdFromArn(arn)
          # example arn format: "arn:aws:codedeploy:us-east-1:123412341234:deployment/12341234-1234-1234-1234-123412341234"
          arn.split(":", 6)[5].split("/",2)[1]
        end
      end
    end
  end
end
