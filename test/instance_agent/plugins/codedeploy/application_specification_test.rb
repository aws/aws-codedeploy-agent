require 'test_helper'
require 'ostruct'
require 'yaml'

module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      module ApplicationSpecification

        class ApplicationSpecificationTest < InstanceAgentTestCase
          def make_app_spec
            ApplicationSpecification.new(YAML.load(@app_spec_string), {:revision_id => @test_revision_id})
          end

          context 'The Application Specification' do
            setup do
              @test_revision_id = 'bar'
            end

            private

            context "With empty AppSpec" do
              setup do
                @app_spec_string = ""
              end

              should "raise an exception" do
                assert_raised_with_message("The deployment failed because the application specification file was empty. Make sure your AppSpec file defines at minimum the 'version' and 'os' properties.", AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With whitespace-only AppSpec" do
              setup do
                @app_spec_string = " \n"
              end

              should "raise an exception" do
                assert_raised_with_message("The deployment failed because the application specification file was empty. Make sure your AppSpec file defines at minimum the 'version' and 'os' properties.", AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With missing version" do
              setup do
                @app_spec_string = <<-END
              os: linux
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because an invalid version value () was entered in the application specification file. Make sure your AppSpec file specifies "0.0" as the version, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With invalid version" do
              setup do
                @app_spec_string = <<-END
              version: invalid
              os: linux
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because an invalid version value (invalid) was entered in the application specification file. Make sure your AppSpec file specifies "0.0" as the version, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With missing os" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because the application specification file specifies an unsupported operating system (). Specify either "linux" or "windows" in the os section of the AppSpec file, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With invalid os" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: unsupported
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because the application specification file specifies an unsupported operating system (unsupported). Specify either "linux" or "windows" in the os section of the AppSpec file, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With no hooks or files" do
              setup do
                @app_spec_string = "version: 0.0\nos: linux\n"
              end
              should "Return an empty hooks hash" do
                app_spec = make_app_spec
                assert_equal({}, app_spec.hooks)
              end
              should "Return an empty files array" do
                app_spec = make_app_spec
                assert_equal([], app_spec.files)
              end
            end

            context "With a single complete hook" do
              setup do
                #A single test script with all parameters
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              hooks:
                test_hook:
                  - location: test_location_1
                    runas: foo
                    timeout: 30
                END
              end
              should "Return a collection containing test script 1" do
                app_spec = make_app_spec
                assert_not_equal nil, app_spec.hooks
                assert_equal ['test_location_1'] , app_spec.hooks["test_hook"].map(&:location)
                assert_equal ['foo'] , app_spec.hooks["test_hook"].map(&:runas)
                assert_equal [30] , app_spec.hooks["test_hook"].map(&:timeout)
              end
            end

            context "With sudo hook" do
              setup do
                #A single test script with all parameters
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              hooks:
                test_hook:
                  - location: test_location_1
                    sudo: true
                    runas: foo
                    timeout: 30
                END
              end
              should "Return a collection containing test script 1" do
                app_spec = make_app_spec
                assert_not_equal nil, app_spec.hooks
                assert_equal ['test_location_1'] , app_spec.hooks["test_hook"].map(&:location)
                assert_equal [true], app_spec.hooks["test_hook"].map(&:sudo)
                assert_equal ['foo'] , app_spec.hooks["test_hook"].map(&:runas)
                assert_equal [30] , app_spec.hooks["test_hook"].map(&:timeout)
              end
            end

            context "With two complete hooks" do
              setup do
                #A pair of test scripts with all parameters
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              hooks:
                test_hook:
                  - location: test_location_1
                    runas: foo
                    timeout: 30
                  - location: test_location_2
                    runas: foo2
                    timeout: 30
                END
              end

              should "Return a collection containing test script 1 and test script 2" do
                app_spec = make_app_spec
                assert_not_equal nil, app_spec.hooks
                assert_equal ['test_location_1', 'test_location_2'] , app_spec.hooks["test_hook"].map(&:location)
              end
            end

            context "With partial hooks (just a runas)" do
              setup do
                #A test script with just a location
                #A test script with location and runas
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              hooks:
                test_hook:
                  - location: test_location
                  - location: test_location_lr
                    runas: foo3
                END
              end

              should "Return a collection containing the two scripts in order" do
                app_spec = make_app_spec()
                assert_not_equal nil, app_spec.hooks
                assert_equal [nil, 'foo3'] , app_spec.hooks["test_hook"].map(&:runas)
              end
            end

            context "With partial hooks (just a timeout)" do
              setup do
                #A test script with just a location
                #A test script with location and timeout
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              hooks:
                test_hook:
                  - location: test_location
                  - location: test_location_lt
                    timeout: 30
                END
              end

              should "Return a collection containing the two scripts in order" do
                app_spec = make_app_spec()
                assert_not_equal nil, app_spec.hooks
                assert_equal [3600, 30] , app_spec.hooks["test_hook"].map(&:timeout)
              end
            end

            context "With empty hook" do
              setup do
                #A test script without a location
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              hooks:
                empty_test_hook:
                test_hook:
                  - location: test_location
                    timeout: 30
                END
              end

              should "Return ignore empty hook" do
                app_spec = make_app_spec()
                assert_not_equal nil, app_spec.hooks
                assert_equal nil , app_spec.hooks["empty_test_hook"]
                assert_equal ['test_location'] , app_spec.hooks["test_hook"].map(&:location)
              end
            end

            context "With missing location data" do
              setup do
                #A test script without a location
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              hooks:
                test_hook:
                  - timeout: 30
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because the application specification file specifies a script with no location value. Specify the location in the hooks section of the AppSpec file, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With non numerical timeout data" do
              setup do
                #A test script with bad timeout data
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              hooks:
                test_hook:
                  - location: test_location
                    timeout: foo
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because an invalid timeout value was provided for a script in the application specification file. Make corrections in the hooks section of the AppSpec file, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "App spec has a file mapping" do
              context "file map contains a single file" do
                setup do
                  @app_spec_string = <<-END
                version: 0.0
                os: linux
                files:
                  - source: test_source
                    destination: test_destination
                  END
                end

                should "return a hash containing the file mapping objects" do
                  app_spec = make_app_spec
                  assert_not_equal nil, app_spec.files
                  assert_equal ['test_source'] , app_spec.files.map(&:source)
                  assert_equal ['test_destination'] , app_spec.files.map(&:destination)
                end
              end

              context "file map contains multiple files" do
                setup do
                  @app_spec_string = <<-END
                version: 0.0
                os: linux
                files:
                  - source: test_source
                    destination: test_destination
                  - source: test_source_2
                    destination: test_destination_2
                  END
                end

                should  "return a hash containing the file mapping objects" do
                  app_spec = make_app_spec
                  assert_not_equal nil, app_spec.files
                  assert_equal ['test_source', 'test_source_2'] , app_spec.files.map(&:source)
                  assert_equal ['test_destination','test_destination_2'] , app_spec.files.map(&:destination)
                end
              end

              context "file map is missing a destination" do
                setup do
                  @app_spec_string = <<-END
                version: 0.0
                os: linux
                files:
                  - source: test_source
                  END
                end

                should  "raise and AppSpecValidationException" do
                  assert_raised_with_message('The deployment failed because the application specification file specifies only a source file (test_source). Add the name of the destination file to the files section of the AppSpec file, and then try again.',AppSpecValidationException) do
                    make_app_spec()
                  end
                end
              end

              context "file map is missing a source" do
                setup do
                  @app_spec_string = <<-END
                version: 0.0
                os: linux
                files:
                  - destination: test_destination
                  END
                end

                should  "raise and AppSpecValidationException" do
                  assert_raised_with_message('The deployment failed because the application specification file specifies a destination file, but no source file. Update the files section of the AppSpec file, and then try again.',AppSpecValidationException) do
                    make_app_spec()
                  end
                end
              end
            end

            context "With permission without object set" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - pattern: test
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because a permission listed in the application specification file has no object value. Update the permissions section of the AppSpec file, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission pattern of **" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: '/'
                  pattern: '**'
                END
              end

              should "match all objects" do
                app_spec = make_app_spec()
                assert_equal '**', app_spec.permissions[0].pattern
              end
            end

            context "With multiple permissions" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: '/'
                - object: test
                - object: more
                END
              end

              should "match all objects" do
                app_spec = make_app_spec()
                assert_equal 3, app_spec.permissions.length
                assert_equal '/', app_spec.permissions[0].object
                assert_equal "test", app_spec.permissions[1].object
                assert_equal "more", app_spec.permissions[2].object
              end
            end

            context "With permissions with pattern" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: '/'
                  pattern: 'glob'
                END
              end

              should "raise when validated as file permission" do
                app_spec = make_app_spec()
                assert_raised_with_message('The deployment failed because the application specification file includes an object (/) with an invalid pattern (glob), such as a pattern for a file applied to a directory. Correct the permissions section of the AppSpec file, and then try again.',AppSpecValidationException) do
                  app_spec.permissions[0].validate_file_permission
                end
              end
            end

            context "With permissions with except" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: '/'
                  except:
                    - 'glob'
                END
              end

              should "raise when validated as file permission" do
                app_spec = make_app_spec()
                assert_raised_with_message('The deployment failed because the except parameter for a pattern in the permissions section (["glob"]) for the object named / contains an invalid format. Update the AppSpec file, and then try again.',AppSpecValidationException) do
                  app_spec.permissions[0].validate_file_permission
                end
              end
            end

            context "With permissions" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: '/'
                END
              end

              should "not raise when validated as file permission" do
                app_spec = make_app_spec()
                assert_nothing_raised do
                  app_spec.permissions[0].validate_file_permission
                end
              end
            end

            context "With permissions with pattern without file type" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: '/'
                  pattern: 'glob'
                  type:
                    - "directory"
                END
              end

              should "not raise when validated as file permission" do
                app_spec = make_app_spec()
                assert_nothing_raised do
                  app_spec.permissions[0].validate_file_permission
                end
              end
            end

            context "With permissions with acl without default ace" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: '/'
                  acls:
                    - 'user:name:rwx'
                END
              end

              should "be able to validate as a file acl" do
                app_spec = make_app_spec()
                assert_nothing_raised do
                  app_spec.permissions[0].validate_file_acl("test")
                end
              end
            end

            context "With permissions with acl with default ace" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: '/'
                  acls:
                    - 'd:user:name:rwx'
                END
              end

              should "be able to validate as a file acl" do
                app_spec = make_app_spec()
                assert_raised_with_message('The deployment failed because the -d parameter has been specified to apply an acl setting to a file. This parameter is supported for directories only. Update the AppSpec file, and then try again.',RuntimeError) do
                  app_spec.permissions[0].validate_file_acl("test")
                end
              end
            end

            context "With valid permission object" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test'
                  pattern: 'prefix*'
                  except: ['*ReadMe', '*.tmp']
                  type: ['file']
                  owner: 'bob'
                  group: 'dev'
                  mode: 6763
                  acls:
                    - 'u:henry:7'
                    - 'd:g:manager:rw'
                  context:
                    name: 'user_u'
                    type: 'unconfined_t'
                    range: 's3-s5:c0.c7,c13'
                END
              end

              should "match pattern when appropriate" do
                app_spec = make_app_spec()
                permission = app_spec.permissions[0]

                assert permission.matches_pattern?(File.expand_path("test/prefix")), "Should match test/prefix for pattern"
                assert permission.matches_pattern?(File.expand_path("test/prefix_matches")), "Should match test/prefix_matches for pattern"
                assert !permission.matches_pattern?(File.expand_path("test/prefix/does_not_match")), "Should not match test/prefix/does_not_match for pattern"
                assert !permission.matches_pattern?(File.expand_path("testprefix/")), "Should not match testprefix/ for pattern"
                assert !permission.matches_pattern?(File.expand_path("tst/prefix")), "Should not match tst/prefix for pattern"
                assert !permission.matches_pattern?(File.expand_path("test/not_prefix")), "Should not match test/not_prefix for pattern"
              end

              should "match except when appropriate" do
                app_spec = make_app_spec()
                permission = app_spec.permissions[0]

                assert permission.matches_except?(File.expand_path("test/this.tmp")), "Should match test/this.tmp for except"
                assert permission.matches_except?(File.expand_path("test/this_ReadMe")), "Should match test/this_ReadMe for except"
                assert !permission.matches_except?(File.expand_path("test/prefix/does_not_match.tmp")), "Should not match test/prefix/does_not_match.tmp for except"
                assert !permission.matches_except?(File.expand_path("testprefix/")), "Should not match testprefix/ for except"
                assert !permission.matches_except?(File.expand_path("tst/prefix")), "Should not match tst/prefix for except"
                assert !permission.matches_except?(File.expand_path("test/not_match")), "Should not match test/not_match for except"
              end

              should "set fields correctly" do
                app_spec = make_app_spec()
                permission = app_spec.permissions[0]
                assert_equal 'test', permission.object
                assert_equal 'prefix*', permission.pattern
                assert_equal ['*ReadMe', '*.tmp'], permission.except
                assert_equal ['file'], permission.type
                assert_equal 'bob', permission.owner
                assert_equal 'dev', permission.group

                mode = permission.mode
                assert_equal '6763', mode.mode
                assert_equal '3', mode.world
                assert_equal false, mode.world_readable
                assert_equal true, mode.world_writable
                assert_equal true, mode.world_executable
                assert_equal '6', mode.group
                assert_equal true, mode.group_readable
                assert_equal true, mode.group_writable
                assert_equal false, mode.group_executable
                assert_equal '7', mode.owner
                assert_equal true, mode.owner_readable
                assert_equal true, mode.owner_writable
                assert_equal true, mode.owner_executable
                assert_equal true, mode.setuid
                assert_equal true, mode.setgid
                assert_equal false, mode.sticky

                acl = permission.acls
                assert_equal 2, acl.aces.length
                ace = acl.aces[0]
                assert_equal false, ace.default
                assert_equal 'user', ace.type
                assert_equal 'henry', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[1]
                assert_equal true, ace.default
                assert_equal 'group', ace.type
                assert_equal 'manager', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal false, ace.execute

                context = permission.context
                assert_equal 'user_u', context.user
                assert_equal nil, context.role
                assert_equal 'unconfined_t', context.type

                range = context.range
                assert_equal 3, range.low_sensitivity
                assert_equal 5, range.high_sensitivity

                categories = range.categories
                assert_equal 9, categories.length
                [(0..7).to_a,13].flatten!.each do |category|
                  assert_equal true, categories.include?(category), "Unable to find expected category #{category}"
                end
              end
            end

            context "With permission with acl with ace with too few parts" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - '7'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because of a problem with the acls permission settings in the application specification file. Invalid acl entry (7).',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with acl with ace with too many parts" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'd:u:bob:7:rwx'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because of a problem with the acls permission settings in the application specification file. Invalid acl entry (d:u:bob:7:rwx).',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with acl with ace with invalid first part" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'invalid:u:bob:7:rwx'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because of a problem with the acls permission settings in the application specification file. Invalid acl entry (invalid:u:bob:7:rwx).',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with acl with ace with invalid second part" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'd:invalid:bob:7:rwx'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because of a problem with the acls permission settings in the application specification file. Invalid acl entry (d:invalid:bob:7:rwx).',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with acl with ace with default as first and second part" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'd:d:bob:7:rwx'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because of a problem with the acls permission settings in the application specification file. Invalid acl entry (d:d:bob:7:rwx).',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with acl with ace with mask with name" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'mask:name:rwx'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because of a problem with the acls permission settings in the application specification file. Invalid acl entry (mask:name:rwx).',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with acl with ace with other with name" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'd:other:name:rwx'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because of a problem with the acls permission settings in the application specification file. Invalid acl entry (d:other:name:rwx).',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with acl with ace with invalid permission character" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'user:bob:rwxd'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because the access control list (ACL) named user:bob:rwxd in the application specification file contains an invalid character (d). Correct the ACL in the hooks section of the AppSpec file, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with acl with valid ace with 4 parts" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'd:u:bob:rwx'
                    - 'default:g:dev:rw'
                END
              end

              should "generate correct fields" do
                app_spec = make_app_spec()

                acl = app_spec.permissions[0].acls
                assert_equal 2, acl.aces.length

                ace = acl.aces[0]
                assert_equal true, ace.default
                assert_equal 'user', ace.type
                assert_equal 'bob', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[1]
                assert_equal true, ace.default
                assert_equal 'group', ace.type
                assert_equal 'dev', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal false, ace.execute
              end
            end

            context "With permission with acl with valid ace with 3 parts" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'd:bob:rwx'
                    - 'default::rw'
                    - 'm::7'
                    - 'mask::7'
                    - 'g:dev:7'
                    - 'group:dev:7'
                    - 'u:bob:7'
                    - 'user:bob:7'
                    - 'u:mask:7'
                    - 'u:other:7'
                END
              end

              should "generate correct fields" do
                app_spec = make_app_spec()

                acl = app_spec.permissions[0].acls
                assert_equal 10, acl.aces.length

                ace = acl.aces[0]
                assert_equal true, ace.default
                assert_equal 'user', ace.type
                assert_equal 'bob', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[1]
                assert_equal true, ace.default
                assert_equal 'user', ace.type
                assert_equal '', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal false, ace.execute

                ace = acl.aces[2]
                assert_equal false, ace.default
                assert_equal 'mask', ace.type
                assert_equal '', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[3]
                assert_equal false, ace.default
                assert_equal 'mask', ace.type
                assert_equal '', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[4]
                assert_equal false, ace.default
                assert_equal 'group', ace.type
                assert_equal 'dev', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[5]
                assert_equal false, ace.default
                assert_equal 'group', ace.type
                assert_equal 'dev', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[6]
                assert_equal false, ace.default
                assert_equal 'user', ace.type
                assert_equal 'bob', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[7]
                assert_equal false, ace.default
                assert_equal 'user', ace.type
                assert_equal 'bob', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[8]
                assert_equal false, ace.default
                assert_equal 'user', ace.type
                assert_equal 'mask', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[9]
                assert_equal false, ace.default
                assert_equal 'user', ace.type
                assert_equal 'other', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute
              end
            end

            context "With permission with acl with valid ace with 2 parts" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'bob:0'
                    - 'm:7'
                    - 'mask:'
                END
              end

              should "generate correct fields" do
                app_spec = make_app_spec()

                acl = app_spec.permissions[0].acls
                assert_equal 3, acl.aces.length

                ace = acl.aces[0]
                assert_equal false, ace.default
                assert_equal 'user', ace.type
                assert_equal 'bob', ace.name
                assert_equal false, ace.read
                assert_equal false, ace.write
                assert_equal false, ace.execute

                ace = acl.aces[1]
                assert_equal false, ace.default
                assert_equal 'mask', ace.type
                assert_equal '', ace.name
                assert_equal true, ace.read
                assert_equal true, ace.write
                assert_equal true, ace.execute

                ace = acl.aces[2]
                assert_equal false, ace.default
                assert_equal 'mask', ace.type
                assert_equal '', ace.name
                assert_equal false, ace.read
                assert_equal false, ace.write
                assert_equal false, ace.execute
              end
            end

            context "With permission with context with invalid sensitivity range" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's3-s2:c0'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because of a problem with the SELinux range specified (s3-s2:c0) for the context parameter in the permissions section of the application specification file. Make corrections in the permissions section of the AppSpec file, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with missing sensitivity range part" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's3-:c0'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid range part s3-',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With single sensitivity" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    type: 'type'
                    range: 's5'
                END
              end

              should "generate correct fields" do
                app_spec = make_app_spec()

                assert_equal 5, app_spec.permissions[0].context.range.low_sensitivity
                assert_equal 5, app_spec.permissions[0].context.range.high_sensitivity
                assert_equal nil, app_spec.permissions[0].context.range.categories
              end
            end

            context "With permission with context with missing sensitivity" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: ':c0'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid range part :c0',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with missing sensitivity value" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid sensitivity s',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with negative sensitivity value" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's0-s-1'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid sensitivity s-1',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with invalid sensitivity" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 'sd3'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid sensitivity sd3',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with invalid sensitivity 2" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 'd3'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid sensitivity d3',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with invalid category range" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's0:c2.c1'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid category range c2.c1',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with missing category range part" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's0:c2.'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid range part c2.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With valid category" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    type: 'type'
                    range: 's0:c0.c1,c15,c7.c9'
                END
              end

              should "generate correct fields" do
                app_spec = make_app_spec()

                categories = app_spec.permissions[0].context.range.categories
                assert_equal 6, categories.length
                [(0..1).to_a, (7..9).to_a, 15].flatten!.each do |category|
                  assert_equal true, categories.include?(category), "Unable to find expected category #{category}"
                end
              end
            end

            context "With permission with context with missing category" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's0:'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid range part s0:',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with missing category value" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's0:c'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid category c',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with negative category value" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's0:c-1'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid category c-1',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with category value above 1023" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's0:c1024'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid category c1024',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context without type" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                END
              end

              should "raise an exception" do
                assert_raised_with_message("The deployment failed because the application specification file specifies an invalid context type ({\"name\"=>\"name\"}). Update the permissions section of the AppSpec file, and then try again.",AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with invalid category" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's0:cd3'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid category cd3',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with invalid category 2" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's0:d3'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('invalid category d3',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with context with duplicate categories" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  context:
                    name: 'name'
                    type: 'type'
                    range: 's0:c0.c2,c1'
                END
              end

              should "raise an exception" do
                assert_raised_with_message('duplicate categories',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with mode with 5 digits" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  mode: 12345
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because the length of a permissions mode (12345) in the application specification file is invalid. Permissions modes must be between one and four characters long. Update the permissions section of the AppSpec file, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with mode with 2 digits" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  mode: 12
                END
              end

              should "fill in needed zeros" do
                app_spec = make_app_spec()

                mode = app_spec.permissions[0].mode
                assert_equal '012', mode.mode
                assert_equal '2', mode.world
                assert_equal false, mode.world_readable
                assert_equal true, mode.world_writable
                assert_equal false, mode.world_executable
                assert_equal '1', mode.group
                assert_equal false, mode.group_readable
                assert_equal false, mode.group_writable
                assert_equal true, mode.group_executable
                assert_equal '0', mode.owner
                assert_equal false, mode.owner_readable
                assert_equal false, mode.owner_writable
                assert_equal false, mode.owner_executable
                assert_equal false, mode.setuid
                assert_equal false, mode.setgid
                assert_equal false, mode.sticky
              end
            end

            context "With permission with mode with invalid char" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  mode: 12a
                END
              end

              should "raise an exception" do
                assert_raised_with_message('The deployment failed because the permissions mode (12a) in the application specification file contains an invalid character (a). Update the permissions section of the AppSpec file, and then try again.',AppSpecValidationException) do
                  make_app_spec()
                end
              end
            end

            context "With permission with valid modes" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  mode: 7777
                - object: 'test1/'
                  mode: 0000
                - object: 'test2/'
                  mode: 777
                END
              end

              should "generate correct fields" do
                app_spec = make_app_spec()

                mode = app_spec.permissions[0].mode
                assert_equal '7777', mode.mode
                assert_equal '7', mode.world
                assert_equal true, mode.world_readable
                assert_equal true, mode.world_writable
                assert_equal true, mode.world_executable
                assert_equal '7', mode.group
                assert_equal true, mode.group_readable
                assert_equal true, mode.group_writable
                assert_equal true, mode.group_executable
                assert_equal '7', mode.owner
                assert_equal true, mode.owner_readable
                assert_equal true, mode.owner_writable
                assert_equal true, mode.owner_executable
                assert_equal true, mode.setuid
                assert_equal true, mode.setgid
                assert_equal true, mode.sticky

                mode = app_spec.permissions[1].mode
                assert_equal '000', mode.mode
                assert_equal '0', mode.world
                assert_equal false, mode.world_readable
                assert_equal false, mode.world_writable
                assert_equal false, mode.world_executable
                assert_equal '0', mode.group
                assert_equal false, mode.group_readable
                assert_equal false, mode.group_writable
                assert_equal false, mode.group_executable
                assert_equal '0', mode.owner
                assert_equal false, mode.owner_readable
                assert_equal false, mode.owner_writable
                assert_equal false, mode.owner_executable
                assert_equal false, mode.setuid
                assert_equal false, mode.setgid
                assert_equal false, mode.sticky

                mode = app_spec.permissions[2].mode
                assert_equal '777', mode.mode
                assert_equal '7', mode.world
                assert_equal true, mode.world_readable
                assert_equal true, mode.world_writable
                assert_equal true, mode.world_executable
                assert_equal '7', mode.group
                assert_equal true, mode.group_readable
                assert_equal true, mode.group_writable
                assert_equal true, mode.group_executable
                assert_equal '7', mode.owner
                assert_equal true, mode.owner_readable
                assert_equal true, mode.owner_writable
                assert_equal true, mode.owner_executable
                assert_equal false, mode.setuid
                assert_equal false, mode.setgid
                assert_equal false, mode.sticky
              end
            end

            context "When acl is present" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls: []
                END
                app_spec = make_app_spec()
                @acl = app_spec.permissions[0].acls
              end

              should "be able to add and clear additional aces" do
                @acl.add_ace("d:henry:4")
                @acl.clear_additional
              end

              should "be able to get an empty acl" do
                assert_equal [], @acl.get_acl
              end

              should "be able to get added aces in the acl" do
                @acl.add_ace("d:henry:4")
                assert_equal 1, @acl.get_acl.length
                @acl.clear_additional
              end

              should "not be able to get a default ace" do
                assert_equal nil, @acl.get_default_ace
              end

              should "be able to get an added default ace" do
                @acl.add_ace("d:henry:4")
                assert_not_nil @acl.get_default_ace
                @acl.clear_additional
              end

              should "not be able to get a default group ace" do
                assert_equal nil, @acl.get_default_group_ace
              end

              should "be able to get an added default group ace" do
                @acl.add_ace("d:g::4")
                assert_not_nil @acl.get_default_group_ace
                @acl.clear_additional
              end

              should "not have a base named ace" do
                assert !@acl.has_base_named?
              end

              should "have a base named ace when added" do
                @acl.add_ace("bob:4")
                assert @acl.has_base_named?
                @acl.clear_additional
              end

              should "not have a base mask ace" do
                assert !@acl.has_base_mask?
              end

              should "have a base mask ace when added" do
                @acl.add_ace("m:4")
                assert @acl.has_base_mask?
                @acl.clear_additional
              end

              should "not have a default ace" do
                assert !@acl.has_default?
              end

              should "have a default ace when added" do
                @acl.add_ace("d:bob:4")
                assert @acl.has_default?
                @acl.clear_additional
              end

              should "not have a default user ace" do
                assert !@acl.has_default_user?
              end

              should "have a default user ace when added" do
                @acl.add_ace("d::4")
                assert @acl.has_default_user?
                @acl.clear_additional
              end

              should "not have a default group ace" do
                assert !@acl.has_default_group?
              end

              should "have a default group ace when added" do
                @acl.add_ace("d:g::4")
                assert @acl.has_default_group?
                @acl.clear_additional
              end

              should "not have a default other ace" do
                assert !@acl.has_default_other?
              end

              should "have a default other ace when added" do
                @acl.add_ace("d:o:4")
                assert @acl.has_default_other?
                @acl.clear_additional
              end

              should "not have a default named ace" do
                assert !@acl.has_default_named?
              end

              should "have a default named ace when added" do
                @acl.add_ace("d:bob:4")
                assert @acl.has_default_named?
                @acl.clear_additional
              end

              should "not have a default mask ace" do
                assert !@acl.has_default_mask?
              end

              should "have a default mask ace when added" do
                @acl.add_ace("d:m:4")
                assert @acl.has_default_mask?
                @acl.clear_additional
              end
            end

            context "When acl is present with existing aces" do
              setup do
                @app_spec_string = <<-END
              version: 0.0
              os: linux
              permissions:
                - object: 'test/'
                  acls:
                    - 'bob:6'
                    - 'm:6'
                    - 'd:bob:0'
                    - 'd::3'
                    - 'd:g::4'
                    - 'd:o:3'
                    - 'd:m:7'
                END
                app_spec = make_app_spec()
                @acl = app_spec.permissions[0].acls
              end

              should "be able to get the acl" do
                assert_equal 7, @acl.get_acl.length
              end

              should "be able to get default ace" do
                assert_not_nil @acl.get_default_ace
              end

              should "be able to get default group ace" do
                assert_not_nil @acl.get_default_group_ace
              end

              should "have base named ace" do
                assert_not_nil @acl.has_base_named?
              end

              should "have base mask ace" do
                assert_not_nil @acl.has_base_mask?
              end

              should "have default ace" do
                assert_not_nil @acl.has_default?
              end

              should "have default user ace" do
                assert_not_nil @acl.has_default_user?
              end

              should "have default group ace" do
                assert_not_nil @acl.has_default_group?
              end

              should "have default other ace" do
                assert_not_nil @acl.has_default_other?
              end

              should "have default named ace" do
                assert_not_nil @acl.has_default_named?
              end

              should "have default mask ace" do
                assert_not_nil @acl.has_default_mask?
              end
            end
          end

          context "With a ContextInfo" do
            should "with a simple range" do
              info = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::ContextInfo.new({"type"=>"type","range"=>"s3"})
              assert_equal "s3", info.range.get_range
            end

            should "with a complex range" do
              info = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::ContextInfo.new({"type"=>"type","range"=>"s3-s7:c5,c8.c10,c17"})
              assert_equal "s3-s7:c5,c8.c10,c17", info.range.get_range
            end
          end

          context "With a ACEInfo" do
            should "not raise if made internal with base entries" do
              assert_nothing_raised do
                InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AceInfo.new("u::7", true)
                InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AceInfo.new("g::7", true)
                InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AceInfo.new("o::7", true)
              end
            end

            should "raise when not internal and has base user" do
              assert_raised_with_message("The deployment failed because of a problem with the acls permission settings in the application specification file. Use mode to set the base acl entry (u::7). Update the permissions section of the AppSpec file, and then try again.",AppSpecValidationException) do
                InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AceInfo.new("u::7")
              end
            end

            should "raise when not internal and has base group" do
              assert_raised_with_message("The deployment failed because of a problem with the acls permission settings in the application specification file. Use mode to set the base acl entry (g::7). Update the permissions section of the AppSpec file, and then try again.",AppSpecValidationException) do
                InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AceInfo.new("g::7")
              end
            end

            should "raise when not internal and has base other" do
              assert_raised_with_message("The deployment failed because of a problem with the acls permission settings in the application specification file. Use mode to set the base acl entry (o:7). Update the permissions section of the AppSpec file, and then try again.",AppSpecValidationException) do
                InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AceInfo.new("o:7")
              end
            end

            should "be able to get the ace" do
              assert_equal("default:user:bob:rwx", InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AceInfo.new("d:u:bob:7").get_ace)
              assert_equal("mask::---", InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AceInfo.new("m:0").get_ace)
            end
          end
        end
      end
    end
  end
end
