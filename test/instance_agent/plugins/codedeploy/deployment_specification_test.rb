require 'test_helper'
require 'ostruct'
require 'certificate_helper'

class DeploymentSpecificationTest < InstanceAgentTestCase

  def generate_signed_message_for(map)
    message = @cert_helper.sign_message(map.to_json)
    spec = OpenStruct.new({ :payload => message })
    spec.format = "PKCS7/JSON"
    return spec
  end

  context 'The Deployment Specification' do
    setup do
      @cert_helper = CertificateHelper.new
      @deployment_id = SecureRandom.uuid.to_s
      @deployment_group_id = SecureRandom.uuid.to_s
      @deployment_group_name = "TestDeploymentGroup"
      @application_name = "TestApplication"
      @s3Revision = {
        "Bucket" => "mybucket",
        "Key" => "mykey",
        "BundleType" => "tar"
      }
      @revision = {
        "RevisionType" => "S3",
        "S3Revision" => @s3Revision
      }
      @deployment_spec = {
        "ApplicationName" => @application_name,
        "DeploymentId" => @deployment_id,
        "DeploymentGroupName" => @deployment_group_name,
        "DeploymentGroupId" => @deployment_group_id,
        "Revision" => @revision
      }
      
      @packed_message = generate_signed_message_for(@deployment_spec)
      InstanceAgent::Config.init
    end 
   
    context "with JSON format" do
      should "populate the deployment id" do
        parsed_deployment_spec = InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        assert_equal @deployment_id, parsed_deployment_spec.deployment_id
        assert_equal @s3Revision, parsed_deployment_spec.revision
        assert_equal @deployment_group_name, parsed_deployment_spec.deployment_group_name
        assert_equal @application_name, parsed_deployment_spec.application_name
      end
    end

    context "with arn deployment id" do
      setup do
        @deployment_spec = {
          "ApplicationName" => @application_name,
          "DeploymentId" => "arn:aws:codedeploy:region:account:deployment/#{@deployment_id}",
          "DeploymentGroupId" => @deployment_group_id,
          "DeploymentGroupName" => @deployment_group_name,
          "Revision" => @revision
        }
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "populate the Bundle" do
        parsed_deployment_spec = InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        assert_equal @deployment_id, parsed_deployment_spec.deployment_id
        assert_equal @deployment_group_id, parsed_deployment_spec.deployment_group_id
        assert_equal @s3Revision, parsed_deployment_spec.revision
        assert_equal @deployment_group_name, parsed_deployment_spec.deployment_group_name
        assert_equal @application_name, parsed_deployment_spec.application_name
      end
    end

    context "with an unsupported format" do
      setup do
        @packed_message.format = "XML"
      end

      should "raise an exception" do
        assert_raised_with_message("Unsupported DeploymentSpecification format: XML") do
          parsed_deployment_spec = InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with additional data" do
      setup do
        @deployment_spec["AdditionalData"] = "test"
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "populate the Bundle" do
        parsed_deployment_spec = InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        assert_equal @deployment_id, parsed_deployment_spec.deployment_id
        assert_equal @deployment_group_id, parsed_deployment_spec.deployment_group_id
        assert_equal @s3Revision, parsed_deployment_spec.revision
        assert_equal @deployment_group_name, parsed_deployment_spec.deployment_group_name
        assert_equal @application_name, parsed_deployment_spec.application_name
      end
    end

    context "with a nil format" do
      setup do
        @packed_message.format = nil
      end

      should "raise an exception" do
        assert_raised_with_message("Unsupported DeploymentSpecification format: ") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "is nil" do
      setup do
        @packed_message = nil
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Provided deployment spec was nil") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with no deployment id" do
      setup do
        @deployment_spec.delete("DeploymentId")
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Deployment Spec has no DeploymentId") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with null deployment id" do
      setup do
        @deployment_spec["DeploymentId"] = nil
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Deployment Spec has no DeploymentId") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with empty deployment id" do
      setup do
        @deployment_spec["DeploymentId"] = ""
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Deployment Spec has no DeploymentId") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with no instance group id" do
      setup do
        @deployment_spec.delete("DeploymentGroupId")
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Deployment Spec has no DeploymentGroupId") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with null instance group id" do
      setup do
        @deployment_spec["DeploymentGroupId"] = nil
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Deployment Spec has no DeploymentGroupId") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with no deployment group name" do
      setup do
        @deployment_spec.delete("DeploymentGroupName")
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Deployment Spec has no DeploymentGroupName") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with null deployment group name" do
      setup do
        @deployment_spec["DeploymentGroupName"] = nil
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Deployment Spec has no DeploymentGroupName") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with no application name" do
      setup do
        @deployment_spec.delete("ApplicationName")
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Deployment Spec has no ApplicationName") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with null application name" do
      setup do
        @deployment_spec["ApplicationName"] = nil
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Deployment Spec has no ApplicationName") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with empty instance group id" do
      setup do
        @deployment_spec["DeploymentGroupId"] = ""
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Deployment Spec has no DeploymentGroupId") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with no target revision" do
      setup do
        @deployment_spec.delete("Revision")
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message('Must specify a revison') do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with null target revision" do
      setup do
        @deployment_spec["Revision"] = nil
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message('Must specify a revison') do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with empty target revision" do
      setup do
        @deployment_spec["Revision"] = ""
        @packed_message = generate_signed_message_for(@deployment_spec)
      end

      should "raise a runtime exception" do
        assert_raised_with_message("Must specify a revision source") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end
    end

    context "with S3 Revision" do
      should "parse correctly" do
        parsed_deployment_spec = InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        assert_equal @s3Revision, parsed_deployment_spec.revision
      end

      should "raise when Bucket is missing" do
        @s3Revision = {
          "Key" => "mykey",
          "BundleType" => "tar"
        }
        @revision = {
          "RevisionType" => "S3",
          "S3Revision" => @s3Revision
        }
        @deployment_spec = {
          "DeploymentId" => @deployment_id,
          "DeploymentGroupId" => @deployment_group_id,
          "Revision" => @revision,
          "DeploymentGroupName" => @deployment_group_name,
          "ApplicationName" => @application_name
        }
        @packed_message = generate_signed_message_for(@deployment_spec)

        assert_raised_with_message("S3Revision in Deployment Spec must specify Bucket, Key and BundleType") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end

      should "raise when Key is missing" do
        @s3Revision = {
          "Bucket" => "mybucket",
          "BundleType" => "tar"
        }
        @revision = {
          "RevisionType" => "S3",
          "S3Revision" => @s3Revision
        }
        @deployment_spec = {
          "DeploymentId" => @deployment_id,
          "DeploymentGroupId" => @deployment_group_id,
          "Revision" => @revision,
          "DeploymentGroupName" => @deployment_group_name,
          "ApplicationName" => @application_name
        }
        @packed_message = generate_signed_message_for(@deployment_spec)

        assert_raised_with_message("S3Revision in Deployment Spec must specify Bucket, Key and BundleType") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end

      should "raise when BundleType is missing" do
        @s3Revision = {
          "Bucket" => "mybucket",
          "Key" => "mykey"
        }
        @revision = {
          "RevisionType" => "S3",
          "S3Revision" => @s3Revision
        }
        @deployment_spec = {
          "DeploymentId" => @deployment_id,
          "DeploymentGroupId" => @deployment_group_id,
          "Revision" => @revision,
          "DeploymentGroupName" => @deployment_group_name,
          "ApplicationName" => @application_name
        }
        @packed_message = generate_signed_message_for(@deployment_spec)

        assert_raised_with_message("S3Revision in Deployment Spec must specify Bucket, Key and BundleType") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end

      should "raise when bundle type is not a supported format" do
        @s3Revision = {
          "Bucket" => "mybucket",
          "Key" => "mykey",
          "BundleType" => "bar"
        }
        @revision = {
          "RevisionType" => "S3",
          "S3Revision" => @s3Revision
        }
        @deployment_spec = {
          "DeploymentId" => @deployment_id,
          "DeploymentGroupId" => @deployment_group_id,
          "Revision" => @revision,
          "DeploymentGroupName" => @deployment_group_name,
          "ApplicationName" => @application_name
        }
        @packed_message = generate_signed_message_for(@deployment_spec)

        assert_raised_with_message("BundleType in S3Revision must be tar, tgz or zip") do
          InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
        end
      end

      should "raise when JSON submitted as PKCS7/JSON" do
        @packed_message.payload = @deployment_spec.to_json

        assert_raised_with_message("Could not parse the PKCS7: nested asn1 error") do
          begin
            InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.parse(@packed_message)
          rescue ArgumentError => e
            raise e.message
          end
        end
      end
    end
  end
end

