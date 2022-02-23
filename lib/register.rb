require 'rubygems'
require 'win32/service'
include Win32

AGENT_NAME = 'codedeployagent'

#The user should be allowed to set the service for automatic start and let the user
#start the service manually.

unless defined?(Ocra)
  Service.create({
    service_name: AGENT_NAME,
    host: nil,
    service_type: Service::WIN32_OWN_PROCESS,
    description: 'AWS CodeDeploy Host Agent Service',
    start_type: Service::DEMAND_START,
    error_control: Service::ERROR_IGNORE,
    binary_path_name: "#{`echo %cd%`.chomp}\\winagent.exe",
    load_order_group: 'Network',
    dependencies: nil,
    display_name: 'AWS CodeDeploy Host Agent Service'
  })

  Service.configure(:service_name => AGENT_NAME, :delayed_start => false)
  
end
