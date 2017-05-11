# language: en
@codedeploy-local
Feature: Local Deploy using AWS CodeDeploy Local CLI

  Scenario: Doing a sample local deployment using a directory bundle
    Given I have a sample local directory bundle
    When I create a local deployment with my bundle
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host
    And the scripts should have been executed during local deployment

  Scenario: Doing a sample local deployment using a relative directory bundle
    Given I have a sample local relative_directory bundle
    When I create a local deployment with my bundle
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host
    And the scripts should have been executed during local deployment

  Scenario: Doing a sample local deployment using a zip bundle
    Given I have a sample local zip bundle
    When I create a local deployment with my bundle
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host
    And the scripts should have been executed during local deployment

  Scenario: Doing a sample local deployment using a tgz bundle
    Given I have a sample local tgz bundle
    When I create a local deployment with my bundle
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host
    And the scripts should have been executed during local deployment

  Scenario: Doing a sample local deployment using a tar bundle
    Given I have a sample local tar bundle
    When I create a local deployment with my bundle
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host
    And the scripts should have been executed during local deployment

  Scenario: Doing a sample local deployment using an s3 bundle
    Given I have a sample bundle uploaded to s3
    When I create a local deployment with my bundle
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host
    And the scripts should have been executed during local deployment
