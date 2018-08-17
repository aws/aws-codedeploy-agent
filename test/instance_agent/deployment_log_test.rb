require 'test_helper'

class InstanceAgentDeploymentLogTest < InstanceAgentTestCase
  setup do
    InstanceAgent::Config.config[:root_dir] = @dir
    InstanceAgent::Config.config[:program_name] = 'app'
    @log_file = File.join(@dir, 'deployment-logs', "app-deployments.log")
  end

  context 'The instance agent deployment log when no explicit :time_zone config option is given' do
    should 'prints log output with local time' do
      Timecop.freeze(Time.local(2008, 9, 1, 12, 0, 0)) do
        InstanceAgent::DeploymentLog.instance.log("Use local time")
        assert_equal("[2008-09-01 12:00:00.000] Use local time\n", `tail -n 1 #{@log_file}`)
      end
    end
  end

  context 'The instance agent deployment log when :time_zone config option is local' do
    setup do
      InstanceAgent::Config.config[:time_zone] = 'local'
    end

    should 'prints log output with local time' do
      Timecop.freeze(Time.local(2018, 9, 1, 15, 0, 0)) do
        InstanceAgent::DeploymentLog.instance.log("Use local time")
        assert_equal("[2018-09-01 15:00:00.000] Use local time\n", `tail -n 1 #{@log_file}`)
      end
    end
  end

  context 'The instance agent deployment log when :time_zone config option is utc' do
    setup do
      InstanceAgent::Config.config[:time_zone] = 'utc'
    end

    should 'prints log output with UTC ISO8601 time' do
      Timecop.freeze(Time.new(2018, 9, 1, 14, 0, 0, "+10:00")) do
        InstanceAgent::DeploymentLog.instance.log("Use UTC ISO8601 formatted time")
        assert_equal("[2018-09-01T04:00:00.000Z] Use UTC ISO8601 formatted time\n", `tail -n 1 #{@log_file}`)
      end
    end
  end
end
