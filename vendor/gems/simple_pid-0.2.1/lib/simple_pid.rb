# Much of this was extracted from Kenneth Kalmer's excellent
# daemon-kit project on GitHub: http://github.com/kennethkalmer/daemon-kit

require File.dirname(__FILE__) + '/core_ext'

class SimplePid
  def initialize(path)
    @path = path.to_absolute_path
  end
  
  def self.drop(path)
    p = self.new(path)
    if p.exists?
      unless p.running?
        p.cleanup
        p.write!
      end
    else
      p.write!
    end
  end
  
  def self.cleanup(path)
    p = self.new(path)
    if p.running?
      return false
    else
      p.cleanup
      return true
    end
  end
  
  def self.cleanup!(path)
    p = self.new(path)
    p.cleanup if p.exists?
  end

  def exists?
    File.exists?(@path)
  end

  # Returns true if the process is running
  def running?
    return false unless self.exists?

    # Check if process is in existence
    # The simplest way to do this is to send signal '0'
    # (which is a single system call) that doesn't actually
    # send a signal
    begin
      Process.kill(0, self.pid)
      return true
    rescue Errno::ESRCH
      return false
    rescue ::Exception   # for example on EPERM (process exists but does not belong to us)
      return true
    #rescue Errno::EPERM
    #  return false
    end
  end

  # Return the pid contained in the pidfile, or nil
  def pid
    return nil unless self.exists?

    File.open( @path ) { |f| return f.gets.to_i }
  end

  def ensure_stopped!
    if self.running?
      puts "Process already running with id #{self.pid}"
      exit 1
    end
  end

  def cleanup
    begin
      File.delete(@path)
    rescue Errno::ENOENT
      File.delete("/tmp/#{Pathname.new(@path).basename}")
    end
  end
  alias zap cleanup

  def write!
    begin
      File.open(@path, "w") { |f| f.puts Process.pid }
    rescue Errno::ENOENT, Errno::EACCES
      File.open("/tmp/#{Pathname.new(@path).basename}", "w") { |f| f.puts Process.pid }
    end
  end
end