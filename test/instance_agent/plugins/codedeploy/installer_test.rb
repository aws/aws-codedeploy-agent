require 'test_helper'

class CodeDeployPluginInstallerTest < InstanceAgentTestCase

  include InstanceAgent::Plugins::CodeDeployPlugin

  context "The CodeDeploy Plugin's file installer" do

    setup do
      @app_spec = mock("app spec")
      @deployment_archive_dir = "deploy-archive-dir"
      @deployment_instructions_dir = "deploy-instructions-dir"
      @deployment_group_id = "ig1"
      @installer =
        Installer.new(:deployment_archive_dir => @deployment_archive_dir,
                      :deployment_instructions_dir => @deployment_instructions_dir)
    end

    context "when initializing" do

      should "require a deployment archive directory" do
        assert_raise { Installer.new(:deployment_instructions_dir => "otherdir") }
      end

      should "require a deployment instructions directory" do
        assert_raise { Installer.new(:deployment_archive_dir => "somedir") }
      end

    end

    context "deployment archive directory getter" do
      should "return the deployment archive directory from the initializer" do
        assert_equal(@deployment_archive_dir, @installer.deployment_archive_dir)
      end
    end

    context "deployment instructions directory getter" do
      should "return the deployment instructions directory from the initializer" do
        assert_equal(@deployment_instructions_dir, @installer.deployment_instructions_dir)
      end
    end

    context "installing" do

      setup do
        @instruction_builder = mock("instruction builder")

        InstallInstruction
          .stubs(:generate_instructions)
          .yields(@instruction_builder)
          .returns(@instruction_builder)

        @instruction_builder.stubs(:cleanup).returns(nil)
        File.stubs(:exists?).returns(false)
        File.stubs(:exists?).with("deploy-instructions-dir/ig1-cleanup").returns(false)

        @app_spec.stubs(:permissions).returns([])
        @app_spec.stubs(:files).returns([])

        File.stubs(:open)
      end

      context "with an existing cleanup file" do

        setup do
          File.stubs(:exists?).with("deploy-instructions-dir/ig1-cleanup").returns(true)
        end

        should "parse the file, execute the commands and remove the file before generating new install instructions" do
          File.stubs(:read).with("deploy-instructions-dir/ig1-cleanup").returns("CLEANUP!")

          call_sequence = sequence("call sequence")
          cleanup_commands = [mock("first"),
                              mock("second")]
          InstallInstruction
            .stubs(:parse_remove_commands)
            .with("CLEANUP!")
            .returns(cleanup_commands)

          cleanup_commands.each do |cmd|
            cmd.expects(:execute).in_sequence(call_sequence)
          end

          FileUtils
            .expects(:rm)
            .with("deploy-instructions-dir/ig1-cleanup")
            .in_sequence(call_sequence)

          InstallInstruction
            .expects(:generate_instructions)
            .returns(CommandBuilder.new)
            .in_sequence(call_sequence)

          @installer.install(@deployment_group_id, @app_spec)
        end

      end

      context "no files to install" do

        should "generate an empty install instructions file" do
          @app_spec.stubs(:files).returns([])
          @instruction_builder.expects(:copy).never
          @instruction_builder.expects(:mkdir).never

          @installer.install(@deployment_group_id, @app_spec)
        end

      end

      context "files to install" do

        context "regular files" do

          setup do
            @app_spec
              .stubs(:files)
              .returns([stub(:source => "src1",
                             :destination => "dst1"),
                        stub(:source => "src2",
                             :destination => "dst2")])

            File.stubs(:directory?).returns(false)
            File.stubs(:exists?).returns(false)
            File.stubs(:exists?).with(any_of("dst1", "dst2")).returns(true)
            @instruction_builder.stubs(:copy)
          end

          should "generate an entry for each file" do
            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src1", "dst1/src1")
            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src2", "dst2/src2")

            @installer.install(@deployment_group_id, @app_spec)
          end

          should "raise an error if the file already exists" do
            File.stubs(:exists?).with("dst2/src2").returns(true)

            assert_raised_with_message("The deployment failed because a specified file already exists at this location: dst2/src2") do
              @installer.install(@deployment_group_id, @app_spec)
            end
          end

          should "generate a mkdir command if the destination directory does not exist" do
            @app_spec
              .stubs(:files)
              .returns([stub(:source => "src1",
                             :destination => "dst1")])
            File.stubs(:exists?).with("dst1").returns(false)

            command_sequence = sequence("command sequence")
            @instruction_builder
              .expects(:mkdir)
              .with("dst1")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src1", "dst1/src1")
              .in_sequence(command_sequence)

            @installer.install(@deployment_group_id, @app_spec)
          end

          should "generate a mkdir command if the destination directory does not exist (absolute path)" do
            @app_spec
              .stubs(:files)
              .returns([stub(:source => "src1",
                             :destination => "/dst1")])

            command_sequence = sequence("command sequence")
            @instruction_builder
              .expects(:mkdir)
              .with("/dst1")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src1", "/dst1/src1")
              .in_sequence(command_sequence)

            @installer.install(@deployment_group_id, @app_spec)
          end

          should "generate mkdir commands for multiple levels of missing directories" do
            @app_spec
              .stubs(:files)
              .returns([stub(:source => "src1",
                             :destination => "dst1/foo/bar")])
            File.stubs(:exists?).with("dst1").returns(true)

            command_sequence = sequence("command sequence")
            @instruction_builder
              .expects(:mkdir)
              .with("dst1/foo")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:mkdir)
              .with("dst1/foo/bar")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src1", "dst1/foo/bar/src1")
              .in_sequence(command_sequence)

            @installer.install(@deployment_group_id, @app_spec)
          end

        end # "regular files"

        context "directories" do

          setup do
            @app_spec
              .stubs(:files)
              .returns([stub(:source => "src1",
                             :destination => "dst1")])

            File.stubs(:directory?).returns(false)
            File.stubs(:directory?).with("deploy-archive-dir/src1").returns(true)
            Dir.stubs(:entries)
              .with("deploy-archive-dir/src1")
              .returns([".", ".."])
            @command_sequence = sequence("commands")
          end

          should "generate a mkdir if the destination doesn't exist" do
            @instruction_builder
              .expects(:mkdir)
              .with("dst1")

            @installer.install(@deployment_group_id, @app_spec)
          end

          should "generate mkdirs multiple levels of missing parent directories (absolute path)" do
            @app_spec
              .stubs(:files)
              .returns([stub(:source => "src1",
                             :destination => "/dst1/foo/bar")])

            File.stubs(:exists?).with("/dst1").returns(true)

            command_sequence = sequence("command sequence")
            @instruction_builder
              .expects(:mkdir)
              .with("/dst1/foo")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:mkdir)
              .with("/dst1/foo/bar")
              .in_sequence(command_sequence)

            @installer.install(@deployment_group_id, @app_spec)
          end

          should "not generate a mkdir if the destination exists" do
            File.stubs(:directory?).with("dst1").returns(true)
            @instruction_builder.expects(:mkdir).never
            @installer.install(@deployment_group_id, @app_spec)
          end

          should "generate a copy after the mkdir for each entry in the directory" do
            Dir.stubs(:entries)
              .with("deploy-archive-dir/src1")
              .returns([".", "..", "foo", "bar"])

            @instruction_builder.expects(:mkdir).with("dst1").in_sequence(@command_sequence)
            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src1/foo", "dst1/foo")
              .in_sequence(@command_sequence)
            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src1/bar", "dst1/bar")
              .in_sequence(@command_sequence)

            @installer.install(@deployment_group_id, @app_spec)
          end

          should "raise an error if an entry already exists" do
            Dir.stubs(:entries)
              .with("deploy-archive-dir/src1")
              .returns([".", "..", "foo", "bar"])
            File.stubs(:exists?).with("dst1/bar").returns(true)
            @instruction_builder.stubs(:mkdir)
            @instruction_builder.stubs(:copy)

            assert_raised_with_message("The deployment failed because a specified file already exists at this location: dst1/bar") do
              @installer.install(@deployment_group_id, @app_spec)
            end
          end

          context "with subdirectories" do

            setup do
              File.stubs(:directory?).with("deploy-archive-dir/src1/foo").returns(true)
              File.stubs(:directory?).with("dst1").returns(true)
              Dir.stubs(:entries)
                .with("deploy-archive-dir/src1")
                .returns([".", "..", "foo"])
              Dir.stubs(:entries)
                .with("deploy-archive-dir/src1/foo")
                .returns([".", ".."])
            end

            should "generate a mkdir for the subdirectory if it doesn't exist" do
              @instruction_builder
                .expects(:mkdir)
                .with("dst1/foo")

              @installer.install(@deployment_group_id, @app_spec)
            end

            should "not generate a mkdir for the subdirectory if it exists" do
              File.stubs(:directory?).with("dst1/foo").returns(true)

              @instruction_builder
                .expects(:mkdir)
                .with("dst1/foo")
                .never

              @installer.install(@deployment_group_id, @app_spec)
            end

            should "generate a copy for each entry in the subdirectory" do
              Dir.stubs(:entries)
                .with("deploy-archive-dir/src1")
                .returns([".", "..", "foo"])
              Dir.stubs(:entries)
                .with("deploy-archive-dir/src1/foo")
                .returns([".", "..", "bar"])

              @instruction_builder
                .expects(:mkdir)
                .with("dst1/foo")
                .in_sequence(@command_sequence)
              @instruction_builder
                .expects(:copy)
                .with("deploy-archive-dir/src1/foo/bar", "dst1/foo/bar")
                .in_sequence(@command_sequence)

              @installer.install(@deployment_group_id, @app_spec)
            end

            should "raise an error if the entry already exists" do
              File.stubs(:exists?).with("dst1/foo/bar").returns(true)
              Dir.stubs(:entries)
                .with("deploy-archive-dir/src1")
                .returns([".", "..", "foo"])
              Dir.stubs(:entries)
                .with("deploy-archive-dir/src1/foo")
                .returns([".", "..", "bar"])
              @instruction_builder.stubs(:mkdir)
              @instruction_builder.stubs(:copy)

              assert_raised_with_message("The deployment failed because a specified file already exists at this location: dst1/foo/bar") do
                @installer.install(@deployment_group_id, @app_spec)
              end
            end

          end # "with subdirectories"

        end # "directories"

        context "with permissions" do
          setup do
            @permissions = [InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("dst1/src1",
                              {:type => ["file"]}),
                            InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("/",
                              {:type => ["directory"],
                               :pattern => "dst*",
                               :except => []}),
                            InstanceAgent::Plugins::CodeDeployPlugin::ApplicationSpecification::LinuxPermissionInfo.new("dst3/",
                              {:type => ["file","directory"],
                               :pattern => "file*",
                               :except => [*2]})]
            @app_spec
              .stubs(:files)
              .returns([stub(:source => "src1",
                             :destination => "dst1"),
                        stub(:source => "src2",
                             :destination => "dst2"),
                        stub(:source => "src3",
                             :destination => "dst3")])
            @app_spec
              .stubs(:permissions)
              .returns(@permissions)

            File.stubs(:directory?).returns(false)
            File.stubs(:directory?).with("deploy-archive-dir/src2").returns(true)
            File.stubs(:directory?).with("deploy-archive-dir/src3").returns(true)
            File.stubs(:directory?).with("deploy-archive-dir/src3/dir1").returns(true)
            File.stubs(:directory?).with("/").returns(true)
            Dir.stubs(:entries)
              .with("deploy-archive-dir/src2")
              .returns([".", ".."])
            Dir.stubs(:entries)
              .with("deploy-archive-dir/src3")
              .returns(["file1", "file2", "dir1", ".", ".."])
            Dir.stubs(:entries)
              .with("deploy-archive-dir/src3/dir1")
              .returns(["file1", ".", ".."])
            File.stubs(:exists?).returns(false)
            File.stubs(:exists?).with(any_of("dst1","dst3")).returns(true)
            @instruction_builder.stubs(:copying_file?).returns(false)
            @instruction_builder.stubs(:copying_file?).with("dst1/src1").returns(true)
            @instruction_builder.stubs(:making_directory?).returns(false)
            @instruction_builder.stubs(:making_directory?).with("dst3/").returns(true)
            @instruction_builder.stubs(:find_matches).returns([])
            @instruction_builder.stubs(:find_matches).with(@permissions[1]).returns(["dst2", "dst3"])
            @instruction_builder.stubs(:find_matches).with(@permissions[2]).returns(["dst3/file1"])
          end

          should "set the permissions" do
            command_sequence = sequence("command sequence")

            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src1", "dst1/src1")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:mkdir)
              .with("dst2")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:mkdir)
              .with("dst3")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src3/file1", "dst3/file1")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src3/file2", "dst3/file2")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:mkdir)
              .with("dst3/dir1")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:copy)
              .with("deploy-archive-dir/src3/dir1/file1", "dst3/dir1/file1")
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:set_permissions)
              .with("dst1/src1", @permissions[0])
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:set_permissions)
              .with("dst2", @permissions[1])
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:set_permissions)
              .with("dst3", @permissions[1])
              .in_sequence(command_sequence)
            @instruction_builder
              .expects(:set_permissions)
              .with("dst3/file1", @permissions[2])
              .in_sequence(command_sequence)

            @installer.install(@deployment_group_id, @app_spec)
          end
        end

      end # "files to install"

      context "after generating instructions" do

        setup do
          @command_list = [stub("a command", :execute => nil),
                           stub("a second command", :execute => nil)]
          commands = mock("command builder")
          commands.stubs(:command_array).returns(@command_list)
          commands.stubs(:cleanup)
          InstallInstruction.stubs(:generate_instructions).returns(commands)
          commands.stubs(:to_json).returns("INSTALL!")
        end

        should "write them to disk" do
          install_file = mock("install file")
          File
            .expects(:open)
            .with("deploy-instructions-dir/ig1-install.json", "w")
            .yields(install_file)
          install_file.expects(:write).with("INSTALL!")

          @installer.install(@deployment_group_id, @app_spec)
        end

        should "execute them after writing to disk" do
          call_sequence = sequence("call sequence")
          File
            .expects(:open)
            .with("deploy-instructions-dir/ig1-install.json", "w")
            .in_sequence(call_sequence)

          cleanup_file = stub("cleanup file")
          File
            .stubs(:open)
            .with("deploy-instructions-dir/ig1-cleanup", "w")
            .yields(cleanup_file)

          @command_list.each do |cmd|
            cmd
              .expects(:execute)
              .with(cleanup_file)
              .in_sequence(call_sequence)
          end

          @installer.install(@deployment_group_id, @app_spec)
        end

      end # "after generating instructions"

    end # "installing"

  end

end
