require 'spec_helper'
require_relative '../../../../vendor/gems/process_manager-0.0.13/lib/process_manager/master'
require 'fileutils'

describe ProcessManager::Daemon::Master do 
	describe "check status" do
  		context "PID file is empty" do
  			it "status is nil and PID file is deleted" do
                 # Make directory
                 file_name = ProcessManager::Daemon::Master.pid_file

                 dirname = File.dirname(file_name)
                 unless File.directory?(dirname)
                 	FileUtils.mkdir_p(dirname)
                 end

                 # Write empty file
                 out_file = File.new(file_name, "w")
                 out_file.close

                 # Check that status is equal to nil and that the PID file is deleted
                 # Note: This used to give a status of 0 but we want it to be nil
                 expect(ProcessManager::Daemon::Master.status).to eq(nil)
                 expect(File.exist?(file_name)).to eq(false)

                 # Clean up directory
                 FileUtils.remove_dir(dirname) if File.directory?(dirname)
             end
         end

         context "PID file has a process that is running" do
         	it "status is the PID number" do
                 # Make directory
                 file_name = ProcessManager::Daemon::Master.pid_file

                 dirname = File.dirname(file_name)
                 unless File.directory?(dirname)
                 	FileUtils.mkdir_p(dirname)
                 end

                 # Write empty file
                 out_file = File.new(file_name, "w")
                 File.write(file_name, $$) # Using $$ to mock a running process
                 out_file.close

                 expect(ProcessManager::Daemon::Master.status).to eq($$)

                 # Clean up and delete the file and directory
                 File.delete(file_name) if File.exist?(file_name)
                 FileUtils.remove_dir(dirname) if File.directory?(dirname)
             end
         end
     end
end