class OnPremiseConfigTest < InstanceAgentTestCase

  include InstanceAgent::Plugins::CodeDeployPlugin

  linux_path = '/etc/codedeploy-agent/conf/codedeploy.onpremises.yml'

  context "Config file doesn't exist" do
    setup do
      File.stubs(:exists?).with(linux_path).returns(false)
    end

    should "do nothing" do
      OnPremisesConfig.configure
    end
  end

  context "Linux config file exists" do
    setup do
      File.stubs(:exists?).with(linux_path).returns(true)
    end

    context "Linux file is not readable" do
      setup do
        File.stubs(:readable?).with(linux_path).returns(false)
      end

      should "do nothing" do
        OnPremisesConfig.configure
      end
    end
    
    context "Linux file is readable" do
      setup do
        File.stubs(:readable?).with(linux_path).returns(true)
      end
      
      context "Linux file is valid" do
        
        linux_file = <<-END
        region: us-east-test
        aws_access_key_id: linuxkey
        aws_secret_access_key: linuxsecretkey
        iam_user_arn: test:arn
        END
  
        setup do
          File.stubs(:read).with(linux_path).returns(linux_file)
        end
  
        should "set the ENV variables correctly" do
          OnPremisesConfig.configure
          assert_equal 'us-east-test', ENV['AWS_REGION']
          assert_equal 'linuxkey', ENV['AWS_ACCESS_KEY']
          assert_equal 'linuxsecretkey', ENV['AWS_SECRET_KEY']
          assert_equal 'test:arn', ENV['AWS_HOST_IDENTIFIER']
        end
      end
      
      context "config file with invalid yaml" do
        linux_file = <<-END
          invalid yaml content
        END
  
        setup do 
          File.stubs(:read).with(linux_path).returns(linux_file)
        end
        
        should "raise an exception" do
          assert_raise do
            OnPremisesConfig.configure
          end
        end
      end
  
      context "config file with session configuration" do
        linux_file = <<-END
        region: us-east-test
        iam_session_arn: test:arn
        aws_credentials_file: /etc/codedeploy-agent/conf/.aws_credentials
        END
  
        setup do
          File.stubs(:read).with(linux_path).returns(linux_file)
        end
  
        should "set the ENV variables correctly" do
          OnPremisesConfig.configure
          assert_equal 'us-east-test', ENV['AWS_REGION']
          assert_equal 'test:arn', ENV['AWS_HOST_IDENTIFIER']
          assert_equal '/etc/codedeploy-agent/conf/.aws_credentials', ENV['AWS_CREDENTIALS_FILE']
        end
      end
      
      context "config file with both session and user arns" do
        linux_file = <<-END
        region: us-east-test
        iam_session_arn: test:arn
        aws_credentials_file: /etc/codedeploy-agent/conf/.aws_credentials
        aws_access_key_id: linuxkey
        aws_secret_access_key: linuxsecretkey
        iam_user_arn: test:arn
        END
        
        setup do
          File.stubs(:read).with(linux_path).returns(linux_file)
        end
        
        should "raise an exception" do 
          assert_raise do
            OnPremisesConfig.configure
          end
        end
      end
      
      context "config file missing region" do
        linux_file = <<-END
        aws_access_key_id: linuxkey
        aws_secret_access_key: linuxsecretkey
        iam_user_arn: test:arn
        END
  
        setup do
          File.stubs(:read).with(linux_path).returns(linux_file)
        end
  
        should "raise an exception" do 
          assert_raise do
            OnPremisesConfig.configure
          end
        end
      end
    end
  end
end
