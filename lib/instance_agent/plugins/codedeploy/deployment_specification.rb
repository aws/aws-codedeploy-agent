require 'openssl'
require 'instance_metadata'
require 'open-uri'

module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      class DeploymentSpecification
        attr_accessor :deployment_id, :deployment_group_id, :deployment_group_name, :revision, :revision_source, :application_name
        attr_accessor :bucket, :key, :bundle_type, :version, :etag
        attr_accessor :external_account, :repository, :commit_id, :anonymous, :external_auth_token
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

          raise 'Must specify a revison' unless data["Revision"]
          @revision_source = data["Revision"]["RevisionType"]
          raise 'Must specify a revision source' unless @revision_source

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
          else
            raise 'Exactly one of S3Revision or GitHubRevision must be specified'
          end
        end

        def self.parse(envelope)
          raise 'Provided deployment spec was nil' if envelope.nil?

          case envelope.format
          when "PKCS7/JSON"
            pkcs7 = OpenSSL::PKCS7.new(envelope.payload)

            # The PKCS7_NOCHAIN flag tells OpenSSL to ignore any PKCS7 CA chain that might be attached
            # to the message directly and use the certificates from provided one only for validating the.
            # signer's certificate.
            #
            # However, it will allow use the PKCS7 signer certificate provided to validate the signature.
            #
            # http://www.openssl.org/docs/crypto/PKCS7_verify.html#VERIFY_PROCESS
            #
            # The ruby wrapper returns true if OpenSSL returns 1
            raise "Validation of PKCS7 signed message failed" unless pkcs7.verify([], @cert_store, nil, OpenSSL::PKCS7::NOCHAIN)

            signer_certs = pkcs7.certificates
            raise "Validation of PKCS7 signed message failed" unless signer_certs.size == 1
            raise "Validation of PKCS7 signed message failed" unless verify_pkcs7_signer_cert(signer_certs[0])

            deployment_spec = JSON.parse(pkcs7.data)

            sanitized_spec = deployment_spec.clone
            sanitized_spec["GitHubAccessToken"] &&= "REDACTED"
            InstanceAgent::Log.debug("#{self.to_s}: Parse: #{sanitized_spec}")

            return new(deployment_spec)
          else
            raise "Unsupported DeploymentSpecification format: #{envelope.format}"
          end
        end

        private
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

        private
        def valid_bundle_type?(revision)
          revision.nil? || %w(tar zip tgz).any? { |k| revision["BundleType"] == k }
        end

        def self.verify_pkcs7_signer_cert(cert)
          @@region ||= ENV['AWS_REGION'] || InstanceMetadata.region
          
          # Do some minimal cert pinning
          case InstanceAgent::Config.config()[:codedeploy_test_profile]
          when 'beta', 'gamma'
            cert.subject.to_s == "/C=US/ST=Washington/L=Seattle/O=Amazon.com, Inc./CN=codedeploy-signer-integ.amazonaws.com"
          when 'prod'
            cert.subject.to_s == "/C=US/ST=Washington/L=Seattle/O=Amazon.com, Inc./CN=codedeploy-signer-"+@@region+".amazonaws.com"
          else
            raise "Unknown profile '#{Config.config()[:codedeploy_test_profile]}'"
          end
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
