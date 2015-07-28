# language: en
@codedeploy-agent
Feature: Deploy using AWS CodeDeploy Agent

  Scenario: Doing a sample deployment
    Given I have an application
    And I have a deployment group containing a single EC2 AMI with preinstalled new instance agent
    When I create a deployment for the application and deployment group with the test S3 revision
    Then the overall deployment should eventually be in progress
    And the deployment should contain all the instances I tagged
    And the overall deployment should eventually succeed