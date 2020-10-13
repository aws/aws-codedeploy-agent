# language: en
@codedeploy-local
Feature: Local Deploy using AWS CodeDeploy Local CLI

  Scenario: Doing a sample local deployment using a directory bundle
    Given I have a sample local directory bundle
    When I create a local deployment with my bundle
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host
    And the scripts should have been executed during local deployment

  Scenario: Doing two sample local deployment using a directory bundle runs correct previous revision scripts
    Given I have a sample local directory bundle
    When I create a local deployment with my bundle
    And I create a local deployment with my bundle
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host twice
    And the scripts should have been executed during two local deployments

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

  Scenario: Doing a sample local deployment using a zipped_directory bundle
    Given I have a sample local zipped_directory bundle
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

  @override-aws-config
  Scenario: Doing a sample local deployment using an s3 bundle
    Given I have a sample bundle uploaded to s3
    When I create a local deployment with my bundle
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host
    And the scripts should have been executed during local deployment

  Scenario: Doing a sample local deployment using a directory bundle with custom event
    Given I have a sample local custom_event_directory bundle
    When I create a local deployment with my bundle with only events ApplicationStart CustomEvent
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host
    And the scripts should have been executed during local deployment with only ApplicationStart CustomEvent

  Scenario: Doing a sample local deployment using a directory bundle with subset of default events
    Given I have a sample local directory bundle
    When I create a local deployment with my bundle with only events BeforeInstall ApplicationStart
    Then the local deployment command should succeed
    And the expected files should have have been locally deployed to my host
    And the scripts should have been executed during local deployment with only BeforeInstall ApplicationStart

  Scenario: Doing a sample local deployment using a directory bundle without file-exists-behavior and existing file
    Given I have a sample local directory_with_destination_files bundle
    And I have existing file in destination
    When I create a local deployment with my bundle with file-exists-behavior MISSING
    Then the local deployment command should fail

  Scenario: Doing a sample local deployment using a directory bundle with file-exists-behavior OVERWRITE
    Given I have a sample local directory_with_destination_files bundle
    And I have existing file in destination
    When I create a local deployment with my bundle with file-exists-behavior OVERWRITE
    Then the local deployment command should succeed
    And the expected existing file should end up like file-exists-behavior OVERWRITE specifies

  Scenario: Doing a sample local deployment using a directory bundle with file-exists-behavior RETAIN
    Given I have a sample local directory_with_destination_files bundle
    And I have existing file in destination
    When I create a local deployment with my bundle with file-exists-behavior RETAIN
    Then the local deployment command should succeed
    And the expected existing file should end up like file-exists-behavior RETAIN specifies
