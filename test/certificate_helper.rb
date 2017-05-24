require 'openssl'
require 'tempfile'

class CertificateHelper
  def initialize()
    generate_root()
    generate_intermediate()
    generate_signer()

    ca_chain_file = generate_ca_chain()

    ENV['AWS_REGION'] = 'us-east-1'
    InstanceAgent::Plugins::CodeDeployPlugin::DeploymentSpecification.init_cert_store(ca_chain_file)
  end

  def generate_root()
    @root_key = OpenSSL::PKey::RSA.new(1024)
    @root_cert = OpenSSL::X509::Certificate.new
    @root_cert.version = 2
    @root_cert.serial = 1
    @root_cert.subject = OpenSSL::X509::Name.new [
        ['C', 'US'], ['ST', 'Washington'], ['L', 'Seattle'],
        ['O', 'Amazon.com, Inc.'], ['CN', 'Host Agent TEST CA Root G1']
    ]
    @root_cert.issuer = @root_cert.subject
    @root_cert.public_key = @root_key.public_key
    @root_cert.not_before = Time.now - 10000
    @root_cert.not_after = Time.now + 10000
    ef = OpenSSL::X509::ExtensionFactory.new

    ef.subject_certificate = @root_cert
    ef.issuer_certificate = @root_cert
    @root_cert.extensions = [
      ef.create_extension("basicConstraints","CA:TRUE",true),
      ef.create_extension("keyUsage","keyCertSign, cRLSign", true),
      ef.create_extension("subjectKeyIdentifier","hash",false),
    ]

    @root_cert.sign(@root_key, OpenSSL::Digest::SHA1.new)

    return @root_cert
  end

  def generate_intermediate()
    @intermediate_key = OpenSSL::PKey::RSA.new(1024)
    @intermediate_cert = OpenSSL::X509::Certificate.new
    @intermediate_cert.version = 2
    @intermediate_cert.serial = 1
    @intermediate_cert.subject = OpenSSL::X509::Name.new [
        ['C', 'US'], ['ST', 'Washington'], ['L', 'Seattle'],
        ['O', 'Amazon.com, Inc.'], ['CN', 'Host Agent TEST CA Intermediate G1']
    ]
    @intermediate_cert.issuer = @root_cert.subject
    @intermediate_cert.public_key = @intermediate_key.public_key
    @intermediate_cert.not_before = Time.now - 10000
    @intermediate_cert.not_after = Time.now + 10000

    ef = OpenSSL::X509::ExtensionFactory.new
    ef.subject_certificate = @intermediate_cert
    ef.issuer_certificate = @root_cert
    @intermediate_cert.extensions = [
      ef.create_extension("basicConstraints","CA:TRUE",true),
      ef.create_extension("keyUsage","keyCertSign, cRLSign", true),
      ef.create_extension("subjectKeyIdentifier","hash",false)
    ]

    @intermediate_cert.sign(@root_key, OpenSSL::Digest::SHA1.new)

    return @intermediate_cert
  end

  def generate_signer()
    @signer_key = OpenSSL::PKey::RSA.new(1024)
    @signer_cert = OpenSSL::X509::Certificate.new
    @signer_cert.version = 2
    @signer_cert.serial = 1
    @signer_cert.subject = OpenSSL::X509::Name.new [
        ['C', 'US'], ['ST', 'Washington'], ['L', 'Seattle'],
        ['O', 'Amazon.com, Inc.'], ['CN', 'codedeploy-signer-us-east-1.amazonaws.com']
    ]
    @signer_cert.issuer = @intermediate_cert.subject
    @signer_cert.public_key = @signer_key.public_key
    @signer_cert.not_before = Time.now - 10000
    @signer_cert.not_after = Time.now + 10000

    ef = OpenSSL::X509::ExtensionFactory.new
    ef.subject_certificate = @signer_cert
    ef.issuer_certificate = @intermediate_cert
    @signer_cert.extensions = [
      ef.create_extension('basicConstraints', 'CA:FALSE', true),
      ef.create_extension('keyUsage', 'keyEncipherment,dataEncipherment,digitalSignature', true),
      ef.create_extension('subjectKeyIdentifier', 'hash')
    ]

    @signer_cert.sign(@intermediate_key, OpenSSL::Digest::SHA1.new)

    return @signer_cert
  end

  def generate_ca_chain()
    ca_chain_file = Tempfile.new('host-agent-deployment-signer-ca-chain')

    File.open(ca_chain_file.path, "wb") do |ca_chain|
      ca_chain.print @root_cert.to_pem
      ca_chain.print @intermediate_cert.to_pem
    end

    return ca_chain_file.path
  end

  def sign_message(message)
    if @signer_key.nil?
      raise "Signer key not initialized"
    end

    pkcs7 = OpenSSL::PKCS7::sign(@signer_cert, @signer_key, message, [], OpenSSL::PKCS7::BINARY)

    return pkcs7.to_pem
  end
end
