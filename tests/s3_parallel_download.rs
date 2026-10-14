//! Tests for the parallel S3 downloader against a fake S3 endpoint that records
//! the requests it receives (count, ranges, peak in flight). Each response is
//! held briefly so concurrent requests really overlap.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use codedeploy_agent::aws_clients::credentials::{CredentialMode, Credentials};
use codedeploy_agent::aws_clients::{S3Client, S3ClientConfig};

/// Position-dependent bytes, so a range at the wrong offset fails comparison.
fn object_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
}

/// How the fake endpoint should behave.
#[derive(Clone, Copy, PartialEq)]
enum ServerMode {
    /// Honor `Range` like S3 does.
    Ranges,
    /// Ignore `Range` and always return the whole object with `200`.
    IgnoreRanges,
    /// Answer any `Range` with `501 NotImplemented`.
    RejectRanges,
    /// Answer any `If-Match` with `412`, as if the object was replaced.
    ObjectChanged,
    /// Send only half of each range's body the first time it is requested.
    TruncateFirstAttempt,
    /// Send only half of every range's body.
    TruncateAlways,
    /// Honor the first range, then answer later ranges with the whole object.
    IgnoreLaterRanges,
    /// Like `RejectRanges`, but the error has no body.
    BareRejectRanges,
    /// Like `ObjectChanged`, but the error has no body.
    BareObjectChanged,
    /// Honor `Range` but send no `ETag`.
    NoEtag,
    /// Honor `Range` and add a wrong full-object checksum header to every 206.
    BadChecksumHeader,
}

/// What the fake endpoint saw.
#[derive(Default)]
struct Observations {
    requests: usize,
    unranged_requests: usize,
    ranges: HashSet<(u64, u64)>,
    /// Every range request, including repeats, so retries are countable.
    range_request_log: Vec<(u64, u64)>,
    peak_in_flight: usize,
    if_match_values: HashSet<String>,
    /// Ranged requests that asked the SDK to validate response checksums.
    ranged_checksum_mode_requests: usize,
    /// Ranges already truncated once.
    truncated: HashSet<(u64, u64)>,
}

struct FakeS3 {
    uri: String,
    observations: Arc<Mutex<Observations>>,
}

impl FakeS3 {
    fn start(body: Vec<u8>, etag: &'static str, mode: ServerMode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake S3");
        let uri = format!("http://{}", listener.local_addr().unwrap());
        let observations = Arc::new(Mutex::new(Observations::default()));

        let body = Arc::new(body);
        let obs = Arc::clone(&observations);
        let in_flight = Arc::new(AtomicUsize::new(0));

        // Detached; the listener lives until the test process exits.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let body = Arc::clone(&body);
                let obs = Arc::clone(&obs);
                let in_flight = Arc::clone(&in_flight);
                std::thread::spawn(move || {
                    handle_connection(stream, &body, etag, mode, &obs, &in_flight);
                });
            }
        });

        Self { uri, observations }
    }

    fn observations(&self) -> std::sync::MutexGuard<'_, Observations> {
        self.observations.lock().unwrap()
    }
}

fn handle_connection(
    mut stream: TcpStream,
    body: &[u8],
    etag: &str,
    mode: ServerMode,
    obs: &Mutex<Observations>,
    in_flight: &AtomicUsize,
) {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let Some(request) = read_request(&mut stream) else {
        return;
    };

    let concurrent = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    {
        let mut obs = obs.lock().unwrap();
        obs.requests += 1;
        obs.peak_in_flight = obs.peak_in_flight.max(concurrent);
        if let Some(if_match) = &request.if_match {
            obs.if_match_values.insert(if_match.clone());
        }
        match request.range {
            Some((start, end)) => {
                obs.ranged_checksum_mode_requests += usize::from(request.checksum_mode);
                obs.ranges.insert((start, end));
                obs.range_request_log.push((start, end));
            },
            None => obs.unranged_requests += 1,
        }
    }

    // Truncate each range once, so its retry succeeds.
    let truncate = match (mode, request.range) {
        (ServerMode::TruncateFirstAttempt, Some(range)) => {
            obs.lock().unwrap().truncated.insert(range)
        },
        (ServerMode::TruncateAlways, Some(_)) => true,
        _ => false,
    };

    // Hold the response briefly so overlapping requests actually overlap. Leave
    // the in-flight count before answering: once the client has the response it
    // may start its next request before this thread runs again.
    std::thread::sleep(Duration::from_millis(50));
    in_flight.fetch_sub(1, Ordering::SeqCst);
    let _ = write_response(&mut stream, body, etag, mode, &request, truncate);
}

struct Request {
    range: Option<(u64, u64)>,
    if_match: Option<String>,
    checksum_mode: bool,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    if line.trim().is_empty() {
        return None;
    }

    let mut range = None;
    let mut if_match = None;
    let mut checksum_mode = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 || header.trim().is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        match name.trim().to_ascii_lowercase().as_str() {
            "range" => range = parse_range_header(&value),
            "if-match" => if_match = Some(value),
            "x-amz-checksum-mode" => checksum_mode = value.eq_ignore_ascii_case("enabled"),
            _ => {},
        }
    }
    Some(Request { range, if_match, checksum_mode })
}

/// `bytes=0-262143` → `(0, 262143)`.
fn parse_range_header(value: &str) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    Some((start.trim().parse().ok()?, end.trim().parse().ok()?))
}

fn write_response(
    stream: &mut TcpStream,
    body: &[u8],
    etag: &str,
    mode: ServerMode,
    request: &Request,
    truncate: bool,
) -> std::io::Result<()> {
    let total = body.len() as u64;

    let etag = (mode != ServerMode::NoEtag).then_some(etag);
    let checksum = mode == ServerMode::BadChecksumHeader;

    if mode == ServerMode::ObjectChanged && request.if_match.is_some() {
        return write_error(stream, 412, "Precondition Failed", Some("PreconditionFailed"));
    }
    if mode == ServerMode::BareObjectChanged && request.if_match.is_some() {
        return write_error(stream, 412, "Precondition Failed", None);
    }
    if mode == ServerMode::BareRejectRanges && request.range.is_some() {
        return write_error(stream, 501, "Not Implemented", None);
    }

    match (mode, request.range) {
        (ServerMode::RejectRanges, Some(_)) => {
            write_error(stream, 501, "Not Implemented", Some("NotImplemented"))
        },
        (ServerMode::IgnoreLaterRanges, Some((start, _))) if start > 0 => {
            write_body(stream, 200, "OK", etag, body, None, false, false)
        },
        (ServerMode::IgnoreRanges, _) | (_, None) => {
            write_body(stream, 200, "OK", etag, body, None, false, false)
        },
        (_, Some((start, end))) => {
            // S3 answers any range on a zero-byte object, or a range that starts
            // past the end, with 416/InvalidRange.
            if start >= total {
                return write_error(
                    stream,
                    416,
                    "Requested Range Not Satisfiable",
                    Some("InvalidRange"),
                );
            }
            let last = end.min(total - 1);
            let slice = &body[usize::try_from(start).unwrap()..=usize::try_from(last).unwrap()];
            let content_range = format!("bytes {start}-{last}/{total}");
            write_body(
                stream,
                206,
                "Partial Content",
                etag,
                slice,
                Some(&content_range),
                truncate,
                checksum,
            )
        },
    }
}

/// `truncate` sends half the promised body and closes the connection.
/// `bad_checksum` adds a CRC32 header that does not match the body.
#[allow(clippy::too_many_arguments)]
fn write_body(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    etag: Option<&str>,
    body: &[u8],
    content_range: Option<&str>,
    truncate: bool,
    bad_checksum: bool,
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nAccept-Ranges: bytes\r\n\
         Content-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(etag) = etag {
        head.push_str(&format!("ETag: \"{etag}\"\r\n"));
    }
    if bad_checksum {
        head.push_str("x-amz-checksum-crc32: AAAAAA==\r\n");
    }
    if let Some(cr) = content_range {
        head.push_str(&format!("Content-Range: {cr}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if truncate {
        stream.write_all(&body[..body.len() / 2])?;
        return stream.flush();
    }
    stream.write_all(body)?;
    stream.flush()
}

/// An S3 XML error, or an empty body when `code` is `None`.
fn write_error(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    code: Option<&str>,
) -> std::io::Result<()> {
    let body = code.map_or_else(String::new, |code| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code>\
             <Message>{reason}</Message><RequestId>fake</RequestId></Error>"
        )
    });
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/xml\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

fn test_credentials() -> Credentials {
    Credentials {
        region: "us-east-1".into(),
        host_identifier: "i-parallel-download".into(),
        mode: CredentialMode::IamUser {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
        },
    }
}

fn client_for(server: &FakeS3, max_concurrency: usize, chunk_size: u64) -> S3Client {
    let config = S3ClientConfig {
        endpoint_override: Some(server.uri.clone()),
        download_max_concurrency: max_concurrency,
        download_chunk_size: chunk_size,
        ..Default::default()
    };
    S3Client::new(test_credentials(), &config).expect("build S3 client")
}

const CHUNK: u64 = 64 * 1024;

#[test]
fn parallel_download_reassembles_object_exactly() {
    // 10 chunks: 9 ranges fan out after the first request.
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 10);
    let server = FakeS3::start(body.clone(), "bundleetag", ServerMode::Ranges);
    let client = client_for(&server, 6, CHUNK);

    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("bundle.zip");
    let etag = client.download_to_file("bucket", "bundle.zip", None, &dest).expect("download");

    assert_eq!(
        etag.as_deref(),
        Some("bundleetag"),
        "ETag must be reported with quotes stripped"
    );
    assert_eq!(std::fs::read(&dest).unwrap(), body, "downloaded bytes must match the object");

    let obs = server.observations();
    assert_eq!(obs.ranges.len(), 10, "expected one request per chunk, saw {:?}", obs.ranges);
    assert!(
        obs.peak_in_flight > 1,
        "downloads were serialized: peak in-flight requests was {}",
        obs.peak_in_flight
    );
    assert!(
        obs.peak_in_flight <= 6,
        "concurrency cap exceeded: {} requests in flight, limit 6",
        obs.peak_in_flight
    );
    // Follow-up ranges send If-Match with the ETag exactly as served (quoted).
    assert_eq!(
        obs.if_match_values.iter().collect::<Vec<_>>(),
        vec!["\"bundleetag\""],
        "expected every follow-up range to send If-Match: \"bundleetag\", saw {:?}",
        obs.if_match_values
    );
}

#[test]
fn parallel_download_respects_concurrency_limit() {
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 8);
    let server = FakeS3::start(body.clone(), "cap", ServerMode::Ranges);
    let client = client_for(&server, 2, CHUNK);

    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("bundle.tgz");
    client.download_to_file("bucket", "bundle.tgz", None, &dest).expect("download");

    assert_eq!(std::fs::read(&dest).unwrap(), body);
    let obs = server.observations();
    assert!(
        obs.peak_in_flight <= 2,
        "max_concurrency=2 must never exceed 2 in flight, saw {}",
        obs.peak_in_flight
    );
}

#[test]
fn small_object_costs_a_single_request() {
    // Smaller than one chunk: one request, no separate size probe.
    let body = object_bytes(1024);
    let server = FakeS3::start(body.clone(), "small", ServerMode::Ranges);
    let client = client_for(&server, 10, CHUNK);

    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("small.zip");
    client.download_to_file("bucket", "small.zip", None, &dest).expect("download");

    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert_eq!(server.observations().requests, 1, "a small object must not cost extra requests");
}

#[test]
fn concurrency_one_uses_a_single_unranged_request() {
    // Concurrency 1: one unranged GetObject.
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 4);
    let server = FakeS3::start(body.clone(), "serial", ServerMode::Ranges);
    let client = client_for(&server, 1, CHUNK);

    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("serial.zip");
    client.download_to_file("bucket", "serial.zip", None, &dest).expect("download");

    assert_eq!(std::fs::read(&dest).unwrap(), body);
    let obs = server.observations();
    assert_eq!(obs.requests, 1, "serial mode must issue exactly one request");
    assert_eq!(obs.unranged_requests, 1, "serial mode must not send a Range header");
}

#[test]
fn endpoint_that_rejects_ranges_falls_back_to_one_request() {
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 4);
    let server = FakeS3::start(body.clone(), "norange", ServerMode::RejectRanges);
    let client = client_for(&server, 8, CHUNK);

    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("fallback.zip");
    let etag = client
        .download_to_file("bucket", "fallback.zip", None, &dest)
        .expect("fallback download");

    assert_eq!(etag.as_deref(), Some("norange"));
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        body,
        "fallback must still produce the whole object"
    );
    assert!(
        server.observations().unranged_requests >= 1,
        "expected an unranged retry after the endpoint refused ranges"
    );
}

#[test]
fn endpoint_that_ignores_ranges_still_downloads_whole_object() {
    // A 200 with the full body must not be truncated to the first chunk.
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 3);
    let server = FakeS3::start(body.clone(), "ignored", ServerMode::IgnoreRanges);
    let client = client_for(&server, 8, CHUNK);

    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("ignored.zip");
    client.download_to_file("bucket", "ignored.zip", None, &dest).expect("download");

    assert_eq!(std::fs::read(&dest).unwrap(), body);
}

#[test]
fn empty_object_downloads_via_fallback() {
    // S3 answers any range on an empty object with 416, so this uses the fallback.
    let server = FakeS3::start(Vec::new(), "empty", ServerMode::Ranges);
    let client = client_for(&server, 8, CHUNK);

    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("empty.zip");
    client.download_to_file("bucket", "empty.zip", None, &dest).expect("download");

    assert_eq!(std::fs::read(&dest).unwrap().len(), 0);
}

#[test]
fn object_replaced_mid_download_fails_loudly() {
    // If-Match must fail the download rather than mix two revisions.
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 4);
    let server = FakeS3::start(body, "stale", ServerMode::ObjectChanged);
    let client = client_for(&server, 4, CHUNK);

    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("changed.zip");
    let err = client
        .download_to_file("bucket", "changed.zip", None, &dest)
        .expect_err("a replaced object must not produce a spliced bundle");

    let message = err.to_string();
    assert!(
        message.contains("changed during the download"),
        "error must name the cause, got: {message}"
    );
    // PreconditionFailed is not retried: at most one request per chunk.
    let obs = server.observations();
    assert!(
        obs.requests <= 4,
        "PreconditionFailed must not be retried, saw {} requests",
        obs.requests
    );
}

#[test]
fn range_body_that_dies_mid_stream_is_retried() {
    // A body cut off mid-stream is retried (the SDK does not retry this).
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 4);
    let server = FakeS3::start(body.clone(), "flaky", ServerMode::TruncateFirstAttempt);
    let client = client_for(&server, 4, CHUNK);

    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("flaky.zip");
    client
        .download_to_file("bucket", "flaky.zip", None, &dest)
        .expect("retry must recover");

    assert_eq!(
        std::fs::read(&dest).unwrap(),
        body,
        "a retried range must overwrite its own offset exactly, leaving no stale half-chunk"
    );

    // Every range was truncated once, so each was requested at least twice.
    let obs = server.observations();
    for range in &obs.ranges {
        let attempts = obs.range_request_log.iter().filter(|r| *r == range).count();
        assert!(attempts >= 2, "range {range:?} was not retried (only {attempts} attempt(s))");
    }
}

#[test]
fn versioned_download_reassembles_object() {
    // Ranged downloads must keep threading versionId through every request.
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 5);
    let server = FakeS3::start(body.clone(), "versioned", ServerMode::Ranges);
    let client = client_for(&server, 5, CHUNK);

    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join("versioned.zip");
    client
        .download_to_file("bucket", "versioned.zip", Some("abc123version"), &dest)
        .expect("download");

    assert_eq!(std::fs::read(&dest).unwrap(), body);
}

fn download(server: &FakeS3, max_concurrency: usize, name: &str) -> (std::io::Result<()>, Vec<u8>) {
    let client = client_for(server, max_concurrency, CHUNK);
    let dir = tempfile::TempDir::new().unwrap();
    let dest = dir.path().join(name);
    let result = client.download_to_file("bucket", name, None, &dest).map(|_| ());
    let bytes = std::fs::read(&dest).unwrap_or_default();
    (result, bytes)
}

#[test]
fn later_range_answered_with_whole_object_fails_without_retrying() {
    // A proxy that drops Range on later requests must not overwrite other ranges.
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 4);
    let server = FakeS3::start(body, "later", ServerMode::IgnoreLaterRanges);
    let (result, _) = download(&server, 4, "later.zip");

    let message = result.expect_err("a 200 for a range must fail").to_string();
    assert!(message.contains("Content-Range"), "{message}");
    assert!(server.observations().requests <= 4, "must not retry a wrong response");
}

#[test]
fn bodiless_501_falls_back_to_one_request() {
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 3);
    let server = FakeS3::start(body.clone(), "bare", ServerMode::BareRejectRanges);
    let (result, bytes) = download(&server, 4, "bare.zip");

    result.expect("a 501 with no body must fall back");
    assert_eq!(bytes, body);
    assert_eq!(server.observations().unranged_requests, 1);
}

#[test]
fn bodiless_412_fails_without_retrying() {
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 4);
    let server = FakeS3::start(body, "bare412", ServerMode::BareObjectChanged);
    let (result, _) = download(&server, 4, "bare412.zip");

    let message = result.expect_err("a replaced object must fail").to_string();
    assert!(message.contains("changed during the download"), "{message}");
    assert!(server.observations().requests <= 4, "a 412 must not be retried");
}

#[test]
fn missing_etag_falls_back_to_one_request() {
    // Nothing would detect the object changing between ranges.
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 3);
    let server = FakeS3::start(body.clone(), "unused", ServerMode::NoEtag);
    let (result, bytes) = download(&server, 4, "noetag.zip");

    result.expect("download");
    assert_eq!(bytes, body);
    let obs = server.observations();
    assert_eq!(obs.ranges.len(), 1, "only the first range should be requested");
    assert_eq!(obs.unranged_requests, 1);
}

#[test]
fn full_object_checksum_header_on_a_range_is_ignored() {
    // A full-object checksum cannot be checked against one range.
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 3);
    let server = FakeS3::start(body.clone(), "crc", ServerMode::BadChecksumHeader);
    let (result, bytes) = download(&server, 4, "crc.zip");

    result.expect("ranged downloads must not validate full-object checksums");
    assert_eq!(bytes, body);
    assert_eq!(
        server.observations().ranged_checksum_mode_requests,
        0,
        "ranged requests must turn off SDK response checksum validation"
    );
}

#[test]
fn whole_object_of_exactly_one_chunk_is_downloaded_again() {
    // No Content-Range and exactly one chunk could be a truncated range.
    let body = object_bytes(usize::try_from(CHUNK).unwrap());
    let server = FakeS3::start(body.clone(), "exact", ServerMode::IgnoreRanges);
    let (result, bytes) = download(&server, 4, "exact.zip");

    result.expect("download");
    assert_eq!(bytes, body);
    assert_eq!(server.observations().requests, 2);
}

#[test]
fn range_that_keeps_failing_gives_up_after_three_attempts() {
    let body = object_bytes(usize::try_from(CHUNK).unwrap() * 3);
    let server = FakeS3::start(body, "broken", ServerMode::TruncateAlways);
    let (result, _) = download(&server, 4, "broken.zip");

    result.expect_err("a range that never completes must fail the download");
    let obs = server.observations();
    for range in &obs.ranges {
        let attempts = obs.range_request_log.iter().filter(|r| *r == range).count();
        // The first range is also requested once by the size probe.
        assert!(attempts <= 4, "range {range:?} requested {attempts} times");
    }
}
