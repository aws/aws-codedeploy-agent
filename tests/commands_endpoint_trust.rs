use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use aws_credential_types::Credentials;
use codedeploy_agent::aws_clients::ssl::verify_tls_connection;
use codedeploy_commands::Client;
use openssl::asn1::Asn1Time;
use openssl::bn::{BigNum, MsbOption};
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::ssl::{SslAcceptor, SslMethod};
use openssl::x509::extension::{
    BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName,
};
use openssl::x509::{X509, X509Builder, X509Name, X509NameBuilder};
use tempfile::TempDir;

const PROBE_URL_VAR: &str = "CODEDEPLOY_AGENT_TLS_PROBE_URL";
const PROBE_TEST: &str = "probe_commands_endpoint_tls";
const PROXY_VARS: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

#[test]
fn commands_endpoint_trusts_partition_ca_from_os_trust_store() {
    let (ca_cert, ca_key) = partition_ca();
    let (server_cert, server_key) = server_certificate(&ca_cert, &ca_key);
    let url = serve_tls(&server_cert, &server_key);
    let trust_store = TempDir::new().unwrap();
    let ca_file = trust_store.path().join("partition-ca.pem");
    std::fs::write(&ca_file, ca_cert.to_pem().unwrap()).unwrap();

    let trusted = run_probe(&url, Some(&ca_file));
    assert!(trusted.contains("precheck: ok") && trusted.contains("poll: ok"), "{trusted}");

    let untrusted = run_probe(&url, None);
    assert!(
        untrusted.contains("precheck: err") && untrusted.contains("poll: err"),
        "{untrusted}"
    );
}

#[test]
#[ignore = "child process of commands_endpoint_trusts_partition_ca_from_os_trust_store"]
fn probe_commands_endpoint_tls() {
    let Ok(url) = std::env::var(PROBE_URL_VAR) else {
        return;
    };
    let client = Client::builder()
        .region("us-isof-east-1")
        .credentials(Credentials::new("probe-key", "probe-secret", None, None, "probe"))
        .endpoint(Some(url.clone()))
        .http_read_timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    println!("precheck: {}", outcome(verify_tls_connection(&url, None)));
    println!("poll: {}", outcome(client.poll_host_command("probe-host")));
}

fn outcome<T, E: std::fmt::Debug>(result: Result<T, E>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("err {e:?}"),
    }
}

fn run_probe(url: &str, ca_file: Option<&Path>) -> String {
    let mut probe = Command::new(std::env::current_exe().unwrap());
    probe
        .args(["--exact", PROBE_TEST, "--ignored", "--nocapture"])
        .env(PROBE_URL_VAR, url)
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR");
    for var in PROXY_VARS {
        probe.env_remove(var);
    }
    if let Some(ca_file) = ca_file {
        probe.env("SSL_CERT_FILE", ca_file);
    }
    let output = probe.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!("{stdout}{stderr}")
}

fn partition_ca() -> (X509, PKey<Private>) {
    let key = ec_key();
    let subject = name("CodeDeploy agent test partition CA");
    let mut builder = certificate_builder(&key, &subject);
    builder.set_issuer_name(&subject).unwrap();
    builder
        .append_extension(BasicConstraints::new().critical().ca().build().unwrap())
        .unwrap();
    builder
        .append_extension(KeyUsage::new().critical().key_cert_sign().crl_sign().build().unwrap())
        .unwrap();
    builder.sign(&key, MessageDigest::sha256()).unwrap();
    (builder.build(), key)
}

fn server_certificate(ca_cert: &X509, ca_key: &PKey<Private>) -> (X509, PKey<Private>) {
    let key = ec_key();
    let mut builder = certificate_builder(&key, &name("localhost"));
    builder.set_issuer_name(ca_cert.subject_name()).unwrap();
    builder
        .append_extension(KeyUsage::new().critical().digital_signature().build().unwrap())
        .unwrap();
    builder
        .append_extension(ExtendedKeyUsage::new().server_auth().build().unwrap())
        .unwrap();
    let san = SubjectAlternativeName::new()
        .dns("localhost")
        .build(&builder.x509v3_context(Some(ca_cert), None))
        .unwrap();
    builder.append_extension(san).unwrap();
    builder.sign(ca_key, MessageDigest::sha256()).unwrap();
    (builder.build(), key)
}

fn certificate_builder(key: &PKey<Private>, subject: &X509Name) -> X509Builder {
    let mut serial = BigNum::new().unwrap();
    serial.rand(64, MsbOption::MAYBE_ZERO, false).unwrap();
    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    builder.set_serial_number(&serial.to_asn1_integer().unwrap()).unwrap();
    builder.set_subject_name(subject).unwrap();
    builder.set_pubkey(key).unwrap();
    builder.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
    builder.set_not_after(&Asn1Time::days_from_now(1).unwrap()).unwrap();
    builder
}

fn name(common_name: &str) -> X509Name {
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_nid(Nid::COMMONNAME, common_name).unwrap();
    name.build()
}

fn ec_key() -> PKey<Private> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
    PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap()
}

fn serve_tls(cert: &X509, key: &PKey<Private>) -> String {
    let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
    acceptor.set_certificate(cert).unwrap();
    acceptor.set_private_key(key).unwrap();
    let acceptor = Arc::new(acceptor.build());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("https://localhost:{}/", listener.local_addr().unwrap().port());
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let acceptor = Arc::clone(&acceptor);
            thread::spawn(move || answer(&acceptor, stream));
        }
    });
    url
}

fn answer(acceptor: &SslAcceptor, stream: TcpStream) {
    let Ok(mut tls) = acceptor.accept(stream) else {
        return;
    };
    read_request(&mut tls);
    let _ = tls.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/x-amz-json-1.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    let _ = tls.shutdown();
}

fn read_request(tls: &mut impl Read) {
    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(read) = tls.read(&mut chunk) {
        if read == 0 {
            return;
        }
        request.extend_from_slice(&chunk[..read]);
        if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..header_end]);
            if request.len() >= header_end + 4 + content_length(&headers) {
                return;
            }
        }
    }
}

fn content_length(headers: &str) -> usize {
    headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
}
