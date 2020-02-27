require 'test_helper'
require 'json'
require 'fileutils'

module InstanceAgent
  module Plugins
    module CodeDeployPlugin
      class InstallInstructionTest < InstanceAgentTestCase
        context "parsing an install file" do
          context "a single mapped file" do
            setup do
              install_instructions = { "revisionId" => "foo" , 'instructions' => [{"type" => :copy, "source" => "test_source", "destination" => "test_destination"}]}
              @parse_string = JSON.dump(install_instructions)
              @mock_file = mock
              @mock_file.stubs(:puts)
              @mock_file.stubs(:size).returns(0)
              @mock_file.stubs(:close)
              File.stubs(:open).returns(@mock_file)
            end

            should "return a collection containing a single CopyCommand which copies from test_source to test_destination" do
              commands = InstallInstruction.parse_install_commands(@parse_string)
              FileUtils.expects(:copy).with("test_source","test_destination", :preserve => true)
              assert_not_equal nil, commands
              commands.each do |command|
                command.execute(@mock_file)
              end
            end
          end

          context "multiple mapped files" do
            setup do
              install_instructions = { "revisionId" => "foo" , "instructions" => [{"type" => "copy", "source" => "test_source", "destination" => "test_destination"}, {"type" => "copy", "source" => "source_2", "destination"=>"destination_2"}]}
              @parse_string = JSON.dump(install_instructions)
              @mock_file = mock
              @mock_file.stubs(:puts)
              @mock_file.stubs(:size).returns(0)
              @mock_file.stubs(:close)
              File.stubs(:open).returns(@mock_file)
            end

            should "return a collection containing multiple copy commands" do
              commands = InstallInstruction.parse_install_commands(@parse_string)
              FileUtils.expects(:copy).with("test_source","test_destination", :preserve => true)
              FileUtils.expects(:copy).with("source_2","destination_2", :preserve => true)
              assert_not_equal nil, commands
              commands.each do |command|
                command.execute(@mock_file)
              end
            end
          end

          context "contains a mkdir command" do
            setup do
              install_instructions = { "revisionId" => "foo" , "instructions" => [{"type" => "copy", "source" => "test_source", "destination" => "test_destination"}, {"type" => "mkdir", "directory"=>"directory"}]}
              @parse_string = JSON.dump(install_instructions)
              @mock_file = mock
              @mock_file.stubs(:puts)
              @mock_file.stubs(:size).returns(0)
              @mock_file.stubs(:close)
              @mock_file.stubs(:exist?).returns(false)
              File.stubs(:open).returns(@mock_file)
              File.stubs(:exists?).returns(false)
            end

            should "return a collection containing a copy command and a mkdir command" do
              commands = InstallInstruction.parse_install_commands(@parse_string)
              FileUtils.expects(:copy).with("test_source","test_destination", :preserve => true)
              FileUtils.expects(:mkdir).with("directory")
              assert_not_equal nil, commands
              commands.each do |command|
                command.execute(@mock_file)
              end
            end
          end

          context "correctly determines method from file type" do
            setup do
              install_instructions = { "revisionId" => "foo" , "instructions" => [{"type" => "copy", "source" => "test_source", "destination" => "test_destination"}, {"type" => "copy", "source" => "source_2", "destination"=>"destination_2"}]}
              @parse_string = JSON.dump(install_instructions)
              @instruction_file = mock
              @instruction_file.stubs(:read).returns(@parse_string)
              @instruction_file.stubs(:path).returns("test/123-install.json")
              @instruction_file.stubs(:close)
              File.stubs(:open).with("test/123-install.json", 'r').returns(@instruction_file)
            end

            should "call parse_install_commands" do
              InstallInstruction.expects(:parse_install_commands).with(@parse_string)
              commands = InstallInstruction.generate_commands_from_file(@instruction_file)
            end
          end

          context "contains a chmod command" do
            setup do
              install_instructions = { "revisionId" => "foo" , "instructions" => [{"type" => "chmod", "mode" => "0740", "file" => "testfile.txt"}]}
              @parse_string = JSON.dump(install_instructions)
              @mock_file = mock
              File.stubs(:chmod)
            end

            should "set the mode of the object" do
              commands = InstallInstruction.parse_install_commands(@parse_string)
              File.expects(:chmod).with("740".to_i(8), "testfile.txt")
              assert_not_equal nil, commands
              commands.each do |command|
                command.execute(@mock_file)
              end
            end
          end

          context "contains a chown command" do
            setup do
              install_instructions = { "revisionId" => "foo" , "instructions" => [{"type" => "chown", "owner" => "bob", "group" => "dev", "file" => "testfile.txt"}]}
              @parse_string = JSON.dump(install_instructions)
              @mock_etc = mock
              @mock_etc.stubs(:gid).returns(222)
              @mock_etc.stubs(:uid).returns(111)
              @mock_file = mock
              Etc.stubs(:getpwnam).with("bob").returns(@mock_etc)
              Etc.stubs(:getgrnam).with("dev").returns(@mock_etc)
              File.stubs(:chchown)
            end

            should "set the owner of the object" do
              commands = InstallInstruction.parse_install_commands(@parse_string)
              File.expects(:chown).with(111, 222, "testfile.txt")
              assert_not_equal nil, commands
              commands.each do |command|
                command.execute(@mock_file)
              end
            end
          end

          context "contains a setfacl command" do
            setup do
              install_instructions = { "revisionId" => "foo" , "instructions" => [{"type" => "setfacl", "acl" => ["user:bob:rwx","default:user:bob:rwx"], "file" => "testfile.txt"}]}
              @parse_string = JSON.dump(install_instructions)
              @mock_file = mock
              @mock_stat = mock
              @mock_stat.stubs(:mode).returns("100421".to_i(8))
              ChangeAclCommand.any_instance.stubs(:system).returns(false)
              @full_acl = "user:bob:rwx,default:user:bob:rwx,user::r--,group::-w-,other::--x,mask::-w-,default:user::r--,default:group::-w-,default:other::--x,default:mask::-w-"
              File.stubs(:stat).returns(@mock_stat)
            end

            should "set the acl of the object" do
              commands = InstallInstruction.parse_install_commands(@parse_string)
              ChangeAclCommand.any_instance.expects(:system).with("setfacl --set #{@full_acl} testfile.txt").returns(true)
              assert_not_equal nil, commands
              commands.each do |command|
                command.execute(@mock_file)
              end
            end

            should "throw if system call fails" do
              commands = InstallInstruction.parse_install_commands(@parse_string)
              ChangeAclCommand.any_instance.expects(:system).with("setfacl --set #{@full_acl} testfile.txt").returns(false)
              assert_not_equal nil, commands
              assert_raise(RuntimeError) do
                commands.each do |command|
                  command.execute(@mock_file)
                end
              end
            end
          end

          context "contains a semanage command" do

            context "with a role" do
              setup do
                install_instructions = { "revisionId" => "foo" , "instructions" => [{"type" => "semanage", "context" => {"name" => "name", "role" => "role", "type" => "type", "range" => "s0" }, "file" => "testfile.txt"}]}
                @parse_string = JSON.dump(install_instructions)
                ChangeContextCommand.any_instance.stubs(:system).returns(true)
                @mock_file = mock
              end

              should "raise an exception" do
                assert_raise(RuntimeError) do
                  commands = InstallInstruction.parse_install_commands(@parse_string)
                end
              end
            end

            context "which is valid" do
              setup do
                install_instructions = { "revisionId" => "foo" , "instructions" => [{"type" => "semanage", "context" => {"name" => "name", "role" => nil, "type" => "type", "range" => "s0" }, "file" => "testfile.txt"}]}
                @parse_string = JSON.dump(install_instructions)
                @mock_file = mock
                ChangeContextCommand.any_instance.stubs(:system).returns(false)
                File.stubs(:realpath).returns("testfile.txt")
              end

              should "set the context of the object" do
                commands = InstallInstruction.parse_install_commands(@parse_string)
                ChangeContextCommand.any_instance.expects(:system).with("semanage fcontext -a -s name -t type -r s0 testfile.txt").returns(true)
                ChangeContextCommand.any_instance.expects(:system).with("restorecon -v testfile.txt").returns(true)
                @mock_file.expects(:puts).with("semanage\0testfile.txt")
                assert_not_equal nil, commands
                commands.each do |command|
                  command.execute(@mock_file)
                end
              end

              should "throw if semanage system call fails" do
                commands = InstallInstruction.parse_install_commands(@parse_string)
                ChangeContextCommand.any_instance.expects(:system).with("semanage fcontext -a -s name -t type -r s0 testfile.txt").returns(false)
                assert_not_equal nil, commands
                assert_raise(RuntimeError) do
                  commands.each do |command|
                    command.execute(@mock_file)
                  end
                end
              end

              should "throw if system call fails" do
                commands = InstallInstruction.parse_install_commands(@parse_string)
                ChangeContextCommand.any_instance.expects(:system).with("semanage fcontext -a -s name -t type -r s0 testfile.txt").returns(true)
                ChangeContextCommand.any_instance.expects(:system).with("restorecon -v testfile.txt").returns(false)
                assert_not_equal nil, commands
                assert_raise(RuntimeError) do
                  commands.each do |command|
                    command.execute(@mock_file)
                  end
                end
              end
            end
          end
        end

        context "Parsing a delete file" do
          context "an empty delete file" do
            setup do
              @parse_string = ""
            end

            should "return an empty command collection" do
              commands = InstallInstruction.parse_remove_commands(@parse_string)
              assert_equal 0, commands.length
            end
          end

          context "a single file to delete" do
            setup do
              @parse_string = "test_delete_path\n"
              File.stubs(:exist?).with("test_delete_path").returns(true)
            end

            should "use rm for a regular file" do
              commands = InstallInstruction.parse_remove_commands(@parse_string)
              FileUtils.expects(:rm).with("test_delete_path")
              commands.each do |command|
                command.execute
              end
            end

            should "use rmdir for a directory" do
              File.stubs(:directory?).with("test_delete_path").returns(true)
              commands = InstallInstruction.parse_remove_commands(@parse_string)
              FileUtils.expects(:rmdir).with("test_delete_path")
              commands.each do |command|
                command.execute
              end
            end

            should "ignore a non-empty directory by rescuing Errno::ENOTEMPTY" do
              File.stubs(:directory?).with("test_delete_path").returns(true)
              commands = InstallInstruction.parse_remove_commands(@parse_string)
              FileUtils.stubs(:rmdir).raises(Errno::ENOTEMPTY)

              assert_nothing_raised do
                commands.each do |command|
                  command.execute
                end
              end
            end
          end

          context "multiple files to delete" do
            setup do
              @parse_string = "test_delete_path\nanother_delete_path\n"
              File.stubs(:directory?).returns(false)
              File.stubs(:exist?).with("test_delete_path").returns(true)
              File.stubs(:exist?).with("another_delete_path").returns(true)
            end

            should "produce a command that deletes test_delete_path" do
              commands = InstallInstruction.parse_remove_commands(@parse_string)
              FileUtils.expects(:rm).with("test_delete_path")
              FileUtils.expects(:rm).with("another_delete_path")
              assert_not_equal nil, commands
              commands.each do |command|
                command.execute
              end
            end

            should "use rmdir for directories" do
              File.stubs(:directory?).with("test_delete_path").returns(true)

              commands = InstallInstruction.parse_remove_commands(@parse_string)
              FileUtils.expects(:rmdir).with("test_delete_path")
              FileUtils.expects(:rm).with("another_delete_path")

              commands.each do |command|
                command.execute
              end
            end
          end

          context "removes mangled line at the end" do
            setup do
              @parse_string = "test_delete_path\nanother_delete_path\nmangled"
              File.stubs(:exist?).with("test_delete_path").returns(true)
              File.stubs(:exist?).with("another_delete_path").returns(true)
            end

            should "produce a command that deletes test_delete_path" do
              commands = InstallInstruction.parse_remove_commands(@parse_string)
              FileUtils.expects(:rm).with("test_delete_path")
              FileUtils.expects(:rm).with("another_delete_path")
              FileUtils.expects(:rm).with("mangled").never
              assert_not_equal nil, commands
              commands.each do |command|
                command.execute
              end
            end
          end

          context "correctly determines method from file type" do
            setup do
              @parse_string = "foo\n"
              @instruction_file = mock
              @instruction_file.stubs(:path).returns("test/123-cleanup")
              File.stubs(:open).with("test/123-cleanup", 'r').returns(@instruction_file)
              @instruction_file.stubs(:read).returns(@parse_string)
              @instruction_file.stubs(:close)
            end

            should "call parse_remove_commands" do
              InstallInstruction.expects(:parse_remove_commands).with(@parse_string)
              commands = InstallInstruction.generate_commands_from_file(@instruction_file)
            end
          end

          context "with a semanage command" do
            setup do
              @parse_string = "semanage\0testfile.txt\n"
              RemoveContextCommand.any_instance.stubs(:system)
            end

            should "remove the context of the object" do
              commands = InstallInstruction.parse_remove_commands(@parse_string)
              RemoveContextCommand.any_instance.expects(:system).with("semanage fcontext -d testfile.txt")
              assert_not_equal nil, commands
              commands.each do |command|
                command.execute
              end
            end
          end
        end

        context "Testing the command builder" do
          setup do
            @command_builder = CommandBuilder.new()
            Dir.chdir Dir.tmpdir()
          end

          should "Have an empty command array" do
            assert_equal @command_builder.command_array, []
          end

          context "with a single copy command" do
            setup do
              @command_builder = CommandBuilder.new()
              @command_builder.copy("source", "destination")
              @expected_json = {"instructions"=>[{"type"=>"copy","source"=>"source","destination"=>"#{File.realdirpath(Dir.tmpdir())}/destination"}]}.to_json
            end

            should "have a single copy in the returned JSON" do
              assert_equal JSON.parse(@expected_json), JSON.parse(@command_builder.to_json)
            end

            should "raise a duplicate exception when a copy collides with another copy" do
              assert_raised_with_message("The deployment failed because the application specification file specifies two source files named source and source for the same destination (#{File.realdirpath(Dir.tmpdir())}/destination). Remove one of the source file paths from the AppSpec file, and then try again.") do
                @command_builder.copy("source", "destination")
              end
            end
          end

          context "with a single mkdir command" do
            setup do
              @command_builder = CommandBuilder.new()
              @command_builder.mkdir("directory")
              @expected_json = {"instructions"=>[{"type"=>"mkdir","directory"=>"#{File.realdirpath(Dir.tmpdir())}/directory"}]}.to_json
            end

            should "have a single mkdir in the returned JSON" do
              assert_equal JSON.parse(@expected_json), JSON.parse(@command_builder.to_json)
            end

            should "raise a duplicate exception when trying to create a directory collides with a copy" do
              @command_builder.copy("source", "directory/dir1")
              assert_raised_with_message("The deployment failed because the application specification file includes an mkdir command more than once for the same destination path (#{File.realdirpath(Dir.tmpdir())}/directory/dir1) from (source). Update the files section of the AppSpec file, and then try again.") do
                @command_builder.mkdir("directory/dir1")
              end
            end
          end

          context "with one of each command (copy and mkdir)" do
            setup do
              @command_builder = CommandBuilder.new()
              @command_builder.mkdir("directory/target")
              @command_builder.copy( "file_target", "directory/target/file_target")
            end

            should "raise a duplicate exception when trying to make a copy collides with a mkdir" do
              assert_raised_with_message("The deployment failed because the application specification file calls for installing the file target, but a file with that name already exists at the location (#{File.realdirpath(Dir.tmpdir())}/directory/target). Update your AppSpec file or directory structure, and then try again.") do
                @command_builder.copy( "target", "directory/target")
              end
            end

            should "say it is copying the appropriate file" do
              assert @command_builder.copying_file?("#{File.realdirpath(Dir.tmpdir())}/directory/target/file_target")
              assert !@command_builder.copying_file?("#{File.realdirpath(Dir.tmpdir())}/directory/target")
            end

            should "say it is making the appropriate directory" do
              assert !@command_builder.making_directory?("#{File.realdirpath(Dir.tmpdir())}/directory/target/file_target")
              assert @command_builder.making_directory?("#{File.realdirpath(Dir.tmpdir())}/directory/target")
            end

            should "match the file when appropriate" do
              permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("#{File.realdirpath(Dir.tmpdir())}/directory/target", {
                :type => ["file"],
                :pattern => "file*",
                :except => []})
              assert @command_builder.find_matches(permission).include?("#{File.realdirpath(Dir.tmpdir())}/directory/target/file_target")

              permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("#{File.realdirpath(Dir.tmpdir())}/directory/target", {
                :type => ["directory"],
                :pattern => "file*",
                :except => []})
              assert !@command_builder.find_matches(permission).include?("#{File.realdirpath(Dir.tmpdir())}/directory/target/file_target")

              permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("#{File.realdirpath(Dir.tmpdir())}/directory/target", {
                :type => ["file"],
                :pattern => "filefile*",
                :except => []})
              assert !@command_builder.find_matches(permission).include?("#{File.realdirpath(Dir.tmpdir())}/directory/target/file_target")

              permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("#{File.realdirpath(Dir.tmpdir())}/directory/target", {
                :type => ["file"],
                :pattern => "file*",
                :except => ["*target"]})
              assert !@command_builder.find_matches(permission).include?("#{File.realdirpath(Dir.tmpdir())}/directory/target/file_target")
            end

            should "match the directory when appropriate" do
              permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("#{File.realdirpath(Dir.tmpdir())}/directory/", {
                :type => ["directory"],
                :pattern => "tar*",
                :except => []})
              assert @command_builder.find_matches(permission).include?("#{File.realdirpath(Dir.tmpdir())}/directory/target")

              permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("#{File.realdirpath(Dir.tmpdir())}/directory/", {
                :type => ["file"],
                :pattern => "tar*",
                :except => []})
              assert !@command_builder.find_matches(permission).include?("#{File.realdirpath(Dir.tmpdir())}/directory/target")

              permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("#{File.realdirpath(Dir.tmpdir())}/directory/", {
                :type => ["directory"],
                :pattern => "tarr*",
                :except => []})
              assert !@command_builder.find_matches(permission).include?("#{File.realdirpath(Dir.tmpdir())}/directory/target")

              permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("#{File.realdirpath(Dir.tmpdir())}/directory/", {
                :type => ["directory"],
                :pattern => "tar*",
                :except => ["*et"]})
              assert !@command_builder.find_matches(permission).include?("#{File.realdirpath(Dir.tmpdir())}/directory/target")
            end
          end

          context "two mkdirs to the same place" do
            setup do
              @command_builder = CommandBuilder.new()
              @command_builder.mkdir("directory")
              @command_builder.mkdir("directory")
              @expected_json = {"instructions"=>[{"type"=>"mkdir","directory"=>"#{File.realdirpath(Dir.tmpdir())}/directory"}]}.to_json
            end

            should "have a single mkdir in the returned JSON" do
              assert_equal JSON.parse(@expected_json), JSON.parse(@command_builder.to_json)
            end
          end

          context "two mkdirs to the same place one has a trailing /" do
            setup do
              @command_builder = CommandBuilder.new()
              @command_builder.mkdir("directory")
              @command_builder.mkdir("directory/")
              @expected_json = {"instructions"=>[{"type"=>"mkdir","directory"=>"#{File.realdirpath(Dir.tmpdir())}/directory"}]}.to_json
            end

            should "have a single mkdir in the returned JSON" do
              assert_equal JSON.parse(@expected_json), JSON.parse(@command_builder.to_json)
            end
          end

          context "setting permissions" do
            should "raise a duplicate exception when trying to set permissions twice" do
              @permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("testfile.txt")
              @command_builder = CommandBuilder.new()
              @command_builder.set_permissions("testfile.txt", @permission)
              assert_raised_with_message("The deployment failed because the permissions setting for (#{File.realdirpath(Dir.tmpdir())}/testfile.txt) is specified more than once in the application specification file. Update the files section of the AppSpec file, and then try again.") do
                @command_builder.set_permissions("testfile.txt", @permission)
              end
            end

            should "not add any commands for empty permissions" do
              @permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("testfile.txt")
              @command_builder = CommandBuilder.new()
              @command_builder.set_permissions("testfile.txt", @permission)
              @expected_json = {"instructions"=>[]}.to_json
              assert_equal JSON.parse(@expected_json), JSON.parse(@command_builder.to_json)
            end

            should "add commands for each part of permisssions" do
              @permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("testfile.txt", {
                :mode=>InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::ModeInfo.new(744),
                :acls=>InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::AclInfo.new(["u:bob:7","d:g:dev:4"]),
                :context=>InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::ContextInfo.new({"name"=>"name","type"=>"type","range"=>"s2-s3:c0,c2.c4,c6"}),
                :owner=>"bob",
                :group=>"dev"})
              @command_builder = CommandBuilder.new()
              @command_builder.set_permissions("testfile.txt", @permission)
              @expected_json = {"instructions"=>[{"type"=>"chmod","mode"=>"744","file"=>"#{File.realdirpath(Dir.tmpdir())}/testfile.txt"},
                {"type"=>"setfacl","acl"=>["user:bob:rwx","default:group:dev:r--"],"file"=>"#{File.realdirpath(Dir.tmpdir())}/testfile.txt"},
                {"type"=>"semanage","context"=>{"user"=>"name","role"=>nil,"type"=>"type","range"=>"s2-s3:c0,c2.c4,c6"},"file"=>"#{File.realdirpath(Dir.tmpdir())}/testfile.txt"},
                {"type"=>"chown","owner"=>"bob","group"=>"dev","file"=>"#{File.realdirpath(Dir.tmpdir())}/testfile.txt"}
                ]}.to_json
              assert_equal JSON.parse(@expected_json), JSON.parse(@command_builder.to_json)
            end

            should "add chown command with just owner" do
              @permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("testfile.txt", {:owner=>"bob"})
              @command_builder = CommandBuilder.new()
              @command_builder.set_permissions("testfile.txt", @permission)
              @expected_json = {"instructions"=>[{"type"=>"chown","owner"=>"bob","group"=>nil,"file"=>"#{File.realdirpath(Dir.tmpdir())}/testfile.txt"}]}.to_json
              assert_equal JSON.parse(@expected_json), JSON.parse(@command_builder.to_json)
            end

            should "add chown command with just group" do
              @permission = InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("testfile.txt", {:group=>"dev"})
              @command_builder = CommandBuilder.new()
              @command_builder.set_permissions("testfile.txt", @permission)
              @expected_json = {"instructions"=>[{"type"=>"chown","owner"=>nil,"group"=>"dev","file"=>"#{File.realdirpath(Dir.tmpdir())}/testfile.txt"}]}.to_json
              assert_equal JSON.parse(@expected_json), JSON.parse(@command_builder.to_json)
            end
          end
        end
      end
    end
  end
end
