//! Behavioral proof that the S3 client honors the YAML `proxy_uri`.
//!
//! Historically the S3 client ignored `proxy_uri` and only read the
//! `HTTP_PROXY`/`HTTPS_PROXY` env vars. This test stands up a local TCP listener
//! that impersonates an HTTP proxy, builds an `S3Client` with `proxy_uri`
//! pointing at it, and triggers a download. If `proxy_uri` is honored, the S3
//! HTTPS request is tunneled as an HTTP `CONNECT <s3-host>:443` to our listener
//! (rather than a direct connection to S3), which we capture and assert on.
//!
//! The download itself fails (our listener is not a real proxy) — we only care
//! that the very first bytes the client sends are a `CONNECT` to an S3 host,
//! which is dispositive: it can only happen if the proxy was applied.

use std::io::Read;
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use codedeploy_agent::aws_clients::credentials::{CredentialMode, Credentials};
use codedeploy_agent::aws_clients::{S3Client, S3ClientConfig};

#[test]
fn s3_client_routes_download_through_proxy_uri() {
    // Bind a throwaway listener on an OS-assigned port; this is our "proxy".
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy listener");
    let proxy_addr = listener.local_addr().unwrap();
    let proxy_uri = format!("http://{proxy_addr}");

    // Capture the first request line the client sends to the "proxy".
    let (tx, rx) = mpsc::channel::<String>();
    let handle = std::thread::spawn(move || {
        listener.set_nonblocking(false).ok();
        if let Ok((mut sock, _)) = listener.accept() {
            sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let mut buf = [0u8; 256];
            let n = sock.read(&mut buf).unwrap_or(0);
            let first = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string();
            let _ = tx.send(first);
        }
    });

    // Build an S3 client with proxy_uri pointing at our listener. Dummy creds +
    // region are fine — we never complete a real request.
    let creds = Credentials {
        mode: CredentialMode::IamUser {
            access_key_id: "AKIAEXAMPLE".into(),
            secret_access_key: "secretexample".into(),
        },
        region: "us-east-1".into(),
        host_identifier: "i-proxytest".into(),
    };
    let config = S3ClientConfig { proxy_uri: Some(proxy_uri.clone()), ..Default::default() };
    let client = S3Client::new(creds, &config).expect("build S3 client");

    // Trigger a download in a background thread — it will fail (fake proxy), but
    // it will first send a CONNECT to our listener if proxy_uri is honored.
    let dl = std::thread::spawn(move || {
        let tmp = std::env::temp_dir().join("s3-proxy-test-out");
        let _ = client.download_to_file("example-bucket", "example-key", None, &tmp);
    });

    // The proxy listener should receive a CONNECT to an S3 host on :443.
    let first_line = rx
        .recv_timeout(Duration::from_secs(15))
        .expect("proxy listener received no connection — proxy_uri was NOT honored");

    let _ = handle.join();
    let _ = dl.join();

    eprintln!("proxy saw first request line: {first_line:?}");
    assert!(
        first_line.starts_with("CONNECT "),
        "expected an HTTP CONNECT tunnel to the proxy, got: {first_line:?}"
    );
    assert!(
        first_line.contains(":443"),
        "expected CONNECT to an HTTPS (:443) target, got: {first_line:?}"
    );
    assert!(
        first_line.to_lowercase().contains("amazonaws.com"),
        "expected CONNECT target to be an S3 host, got: {first_line:?}"
    );
}
