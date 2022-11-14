module InstanceAgent; module Plugins; module CodeDeployPlugin
class CommandAcknowledgementRequestBuilder
  @@MIN_ACK_TIMEOUT = 60
  @@MAX_ACK_TIMEOUT = 4200

  def initialize(logger)
    @logger = logger
  end

  def build(diagnostics, host_command_identifier, timeout)
    result = build_default(diagnostics, host_command_identifier)
    if timeout && timeout > 0
      result[:host_command_max_duration_in_seconds] = correct_timeout(timeout)
    end

    result
  end

  private

  def build_default(diagnostics, host_command_identifier)
    {
      :diagnostics => diagnostics,
      :host_command_identifier => host_command_identifier
    }
  end

  def correct_timeout(timeout)
    result = timeout
    if timeout < @@MIN_ACK_TIMEOUT
      log(:info, "Command timeout of #{timeout} is below minimum value of #{@@MIN_ACK_TIMEOUT} " +
        "seconds. Sending #{@@MIN_ACK_TIMEOUT} to the service instead.")
      result = @@MIN_ACK_TIMEOUT
    elsif timeout > @@MAX_ACK_TIMEOUT
      log(:warn, "Command timeout of #{timeout} exceeds maximum accepted value #{@@MAX_ACK_TIMEOUT} " +
        "seconds. Sending #{@@MAX_ACK_TIMEOUT} to the service instead. Commands may time out.")
      result = @@MAX_ACK_TIMEOUT
    end

    result
  end

  def log(severity, message)
    raise ArgumentError, "Unknown severity #{severity.inspect}" unless InstanceAgent::Log::SEVERITIES.include?(severity.to_s)
    @logger.send(severity.to_sym, "#{self.class.to_s}: #{message}")
  end
end end end end
