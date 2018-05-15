require 'spec_helper'
require 'instance_agent'
require 'instance_agent/config'
require 'instance_agent/plugins/codedeploy/deployment_command_tracker'
require 'instance_agent/log'

describe InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker do       
    describe '.create_ongoing_deployment_tracking_file' do
        $deployment_id = 'D-123'
        deployment_command_tracker = InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker;
        context "when the deployment life cycle event is in progress" do
            before do
                InstanceAgent::Config.config[:root_dir] = File.join(Dir.tmpdir(), 'codeDeploytest')
                InstanceAgent::Config.config[:ongoing_deployment_tracking] = 'ongoing-deployment' 
                InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.create_ongoing_deployment_tracking_file($deployment_id)                
            end
            it 'tries to create ongoing-deployment folder' do 
                directories_in_deployment_root_folder = deployment_command_tracker.directories_and_files_inside(InstanceAgent::Config.config[:root_dir]);
                expect(directories_in_deployment_root_folder).to include(InstanceAgent::Config.config[:ongoing_deployment_tracking]);
            end 
            it 'creates ongoing-deployment file in the tracking folder' do 
                files_in_deployment_tracking_folder = deployment_command_tracker.directories_and_files_inside(File.join(InstanceAgent::Config.config[:root_dir], InstanceAgent::Config.config[:ongoing_deployment_tracking]))
                expect(files_in_deployment_tracking_folder).to include($deployment_id);
            end   
        end
    end
    describe '.check_deployment_event_inprogress' do
        context 'when no deployment life cycle event is in progress' do
            before do 
                InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.delete_deployment_command_tracking_file($deployment_id)
            end
            it 'checks if any deployment event is in progress' do 
                expect(InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.check_deployment_event_inprogress?).to equal(false);
            end
        end
        context 'when deployment life cycle event is in progress' do
            before do
                InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.create_ongoing_deployment_tracking_file($deployment_id)                
            end
            it 'checks if any deployment life cycle event is in progress ' do 
                expect(InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.check_deployment_event_inprogress?).to equal(true)
            end
        end
        context 'when the agent starts for the first time' do
            before do 
                FileUtils.rm_r(File.join(InstanceAgent::Config.config[:root_dir], InstanceAgent::Config.config[:ongoing_deployment_tracking]))              
            end
            it 'checks if any deployment life cycle event is in progress ' do 
                expect(InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.check_deployment_event_inprogress?).to equal(false)
            end
        end     
    end
    describe '.delete_deployment_tracking_file_if_stale' do
        context 'when deployment life cycle event is in progress' do 
            before do
                InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.create_ongoing_deployment_tracking_file($deployment_id)                
            end
            it 'checks if the file is stale or not' do
                expect(InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.delete_deployment_tracking_file_if_stale?($deployment_id, 2000)).to equal(false)
            end
        end
        context 'when the wait-time has been more than the timeout time' do 
            it 'checks if the file is stale after the timeout' do
                sleep 10
                expect(InstanceAgent::Plugins::CodeDeployPlugin::DeploymentCommandTracker.delete_deployment_tracking_file_if_stale?($deployment_id, 5)).to equal(true)
            end
        end
    end   
end    