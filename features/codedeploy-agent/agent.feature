# language: en
@codedeploy-agent
Feature: Deploy using AWS CodeDeploy Agent

  Scenario: Doing a sample deployment
    Given I have a sample bundle uploaded to s3
    And I have a CodeDeploy application
    And I register my host in CodeDeploy
    And I startup the CodeDeploy agent locally
    And I have a deployment group containing my host
    When I create a deployment for the application and deployment group with the test S3 revision
    Then the overall deployment should eventually be in progress
    And the deployment should contain all the instances I tagged
    And the overall deployment should eventually succeed
    And the expected files should have have been deployed to my host
    And the scripts should have been executed
