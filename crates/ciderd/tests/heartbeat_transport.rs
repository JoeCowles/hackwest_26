use anyhow::{ensure, Context, Result};
use ciderd::{
    config::Config,
    heartbeat::{bearer_header, FailureKind, Sender},
    model::{now, Acknowledgement, Heartbeat},
};
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair, KeyUsagePurpose};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
};
use tokio_rustls::{
    rustls::{self, pki_types::PrivatePkcs8KeyDer},
    TlsAcceptor,
};

const TOKEN: &str = "local-integration-secret-never-in-telemetry";

#[derive(Clone, Copy)]
enum Reply {
    Ack,
    WrongSession,
    TooLarge,
    ChunkedTooLarge,
    Redirect,
    Unauthorized,
    RateLimited,
}

struct Request {
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    heartbeat: Heartbeat,
}

struct HttpsPeer {
    endpoint: String,
    ca_path: PathBuf,
    requests: mpsc::Receiver<Request>,
    task: JoinHandle<()>,
    _directory: TempDir,
}
impl Drop for HttpsPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl HttpsPeer {
    async fn start(reply: Reply) -> Self {
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca = CertifiedIssuer::self_signed(ca_params, KeyPair::generate().unwrap()).unwrap();
        let key = KeyPair::generate().unwrap();
        let leaf = CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()])
            .unwrap()
            .signed_by(&key, &ca)
            .unwrap();
        let mut tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![leaf.der().clone()],
                PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            )
            .unwrap();
        tls.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let ca_path = directory.path().join("ca.pem");
        std::fs::write(&ca_path, ca.pem()).unwrap();
        let (send, requests) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let Ok(Ok(mut stream)) =
                    tokio::time::timeout(Duration::from_secs(3), acceptor.accept(socket)).await
                else {
                    continue;
                };
                let Ok(Ok(request)) =
                    tokio::time::timeout(Duration::from_secs(3), read_request(&mut stream)).await
                else {
                    continue;
                };
                let response = make_response(reply, &request.heartbeat);
                if send.send(request).await.is_err() {
                    break;
                }
                if stream.write_all(&response).await.is_err() {
                    continue;
                }
                let _ = stream.shutdown().await;
            }
        });
        Self {
            endpoint: format!("https://{address}/heartbeat"),
            ca_path,
            requests,
            task,
            _directory: directory,
        }
    }
    fn config(&self) -> Config {
        let mut config = Config::parse(include_str!("../examples/ciderd.toml")).unwrap();
        config.heartbeat.endpoint = self.endpoint.clone();
        config.heartbeat.ca_certificate_file = Some(self.ca_path.clone());
        config.heartbeat.connect_timeout_seconds = 1;
        config.heartbeat.request_timeout_seconds = 2;
        config
    }
    async fn received(&mut self) -> Request {
        tokio::time::timeout(Duration::from_secs(4), self.requests.recv())
            .await
            .unwrap()
            .unwrap()
    }
}

async fn read_request<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> Result<Request> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let size = stream.read(&mut buffer).await?;
        ensure!(size > 0, "incomplete HTTP request");
        ensure!(
            bytes.len() + size <= 4 * 1024 * 1024,
            "HTTP request exceeds test bound"
        );
        bytes.extend_from_slice(&buffer[..size]);
        let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") else {
            continue;
        };
        let header = std::str::from_utf8(&bytes[..end])?;
        ensure!(
            header.lines().next() == Some("POST /heartbeat HTTP/1.1"),
            "unexpected method/path/version"
        );
        let headers: BTreeMap<String, String> = header
            .lines()
            .skip(1)
            .map(|line| {
                let (key, value) = line.split_once(':').context("malformed HTTP header")?;
                Ok((key.to_ascii_lowercase(), value.trim().to_owned()))
            })
            .collect::<Result<_>>()?;
        let length: usize = headers
            .get("content-length")
            .context("missing content length")?
            .parse()?;
        ensure!(length <= 4 * 1024 * 1024, "HTTP body exceeds test bound");
        if bytes.len() < end + 4 + length {
            continue;
        }
        let body = bytes[end + 4..end + 4 + length].to_vec();
        let heartbeat: Heartbeat = serde_json::from_slice(&body)?;
        heartbeat.validate()?;
        return Ok(Request {
            headers,
            body,
            heartbeat,
        });
    }
}

fn make_response(reply: Reply, heartbeat: &Heartbeat) -> Vec<u8> {
    if matches!(reply, Reply::TooLarge) {
        return b"HTTP/1.1 200 OK\r\nContent-Length: 65537\r\nConnection: close\r\n\r\n".to_vec();
    }
    if matches!(reply, Reply::ChunkedTooLarge) {
        let mut response =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n400\r\n"
                .to_vec();
        response.extend_from_slice(&vec![b'x'; 1024]);
        response.extend_from_slice(b"\r\n0\r\n\r\n");
        return response;
    }
    if matches!(reply, Reply::Redirect) {
        return b"HTTP/1.1 302 Found\r\nLocation: /redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    }
    if matches!(reply, Reply::Unauthorized) {
        return b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_vec();
    }
    if matches!(reply, Reply::RateLimited) {
        return b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 3\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    }
    let ack = Acknowledgement {
        schema_version: "2.0".into(),
        agent_session_id: if matches!(reply, Reply::WrongSession) {
            "wrong-session".into()
        } else {
            heartbeat.agent_session_id.clone()
        },
        accepted_sequence: heartbeat.sequence,
        inventory_revision: Some(heartbeat.inventory.revision),
        request_inventory: false,
        server_received_at: now(),
    };
    let body = serde_json::to_vec(&ack).unwrap();
    let mut response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).into_bytes();
    response.extend_from_slice(&body);
    response
}

fn fixture() -> Heartbeat {
    serde_json::from_slice(include_bytes!("../contract/example-heartbeat.json")).unwrap()
}

#[tokio::test]
async fn production_sender_trusts_only_explicit_ca_and_sends_sensitive_bearer() {
    let mut peer = HttpsPeer::start(Reply::Ack).await;
    let sender = Sender::new(peer.config().heartbeat).unwrap();
    let heartbeat = fixture();
    let authorization = bearer_header(TOKEN).unwrap();
    let ack = sender.send(&heartbeat, &authorization).await.unwrap();
    assert_eq!(ack.accepted_sequence, heartbeat.sequence);
    let received = peer.received().await;
    assert_eq!(received.heartbeat, heartbeat);
    assert_eq!(received.headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(received.headers["content-type"], "application/json");
    assert!(!String::from_utf8_lossy(&received.body).contains(TOKEN));
    let mut untrusted = peer.config();
    untrusted.heartbeat.ca_certificate_file = None;
    let error = Sender::new(untrusted.heartbeat)
        .unwrap()
        .send(&heartbeat, &authorization)
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::Network);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), peer.requests.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn production_sender_rejects_bad_ack_redirect_and_bounded_response_failures() {
    let heartbeat = fixture();
    let authorization = bearer_header(TOKEN).unwrap();
    for (mode, kind) in [
        (Reply::WrongSession, FailureKind::InvalidAcknowledgement),
        (Reply::TooLarge, FailureKind::ResponseTooLarge),
        (Reply::ChunkedTooLarge, FailureKind::ResponseTooLarge),
        (Reply::Redirect, FailureKind::Contract),
        (Reply::Unauthorized, FailureKind::Authentication),
        (Reply::RateLimited, FailureKind::RateLimited),
    ] {
        let mut peer = HttpsPeer::start(mode).await;
        let mut config = peer.config();
        config.heartbeat.maximum_response_bytes = 512;
        let error = Sender::new(config.heartbeat)
            .unwrap()
            .send(&heartbeat, &authorization)
            .await
            .unwrap_err();
        assert_eq!(error.kind, kind);
        if kind == FailureKind::RateLimited {
            assert_eq!(error.retry_after, Some(Duration::from_secs(3)));
        }
        peer.received().await;
        // A redirect or status error must not cause hidden follow-up attempts.
        assert!(
            tokio::time::timeout(Duration::from_millis(30), peer.requests.recv())
                .await
                .is_err()
        );
    }
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_runtime_keeps_sending_during_stuck_native_workers_and_repeats_observations() {
    use ciderd::{identity, model::WorkerPhase, runtime};
    use std::os::unix::fs::PermissionsExt;
    let mut peer = HttpsPeer::start(Reply::Ack).await;
    let directory = tempfile::tempdir().unwrap();
    let mut config = peer.config();
    config.node.identity_file = directory.path().join("node.json");
    config.node.state_directory = directory.path().join("state");
    let jobs_directory = config.node.state_directory.join("jobs");
    config.heartbeat.bearer_token_file = directory.path().join("token");
    config.heartbeat.interval_seconds = 1;
    config.heartbeat.maximum_backoff_seconds = 2;
    config.collection.jitter_percent = 0;
    config.collection.io_seconds = 30;
    config.collection.mount_inventory_seconds = 30;
    config.collection.inventory_reconcile_seconds = 30;
    config.collection.nfs_counters_seconds = 30;
    config.collection.apfs_accounting_seconds = 30;
    config.limits.native_workers = 2;
    std::fs::write(&config.heartbeat.bearer_token_file, TOKEN).unwrap();
    std::fs::set_permissions(
        &config.heartbeat.bearer_token_file,
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    identity::enroll(&config.node.identity_file, Some("local-tls-test-node")).unwrap();
    // A fixed test-only executable replaces itself with sleep; no descendant is
    // left outside the production supervisor's Child ownership.
    let worker = directory.path().join("stalled-native-worker");
    std::fs::write(&worker, b"#!/bin/sh\nexec /bin/sleep 60\n").unwrap();
    std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (shutdown, stopping) = tokio::sync::oneshot::channel();
    let mut daemon = tokio::spawn(runtime::run_with_worker(config, worker, async {
        let _ = stopping.await;
    }));
    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        let mut requests = Vec::new();
        for _ in 0..5 {
            requests.push(peer.requests.recv().await.context("TLS peer stopped")?);
        }
        Ok::<_, anyhow::Error>(requests)
    })
    .await;
    let _ = shutdown.send(());
    let completion = tokio::time::timeout(Duration::from_secs(6), &mut daemon).await;
    if completion.is_err() {
        daemon.abort();
    }
    completion
        .expect("production runtime shutdown must be bounded")
        .unwrap()
        .unwrap();
    for class in ["command", "native", "remote"] {
        assert!(
            std::fs::read_dir(jobs_directory.join(class))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("job-")),
            "shutdown must confirm worker exit and remove its durable active-job record"
        );
    }
    let requests = observed
        .expect("heartbeat count must advance while native workers are stalled")
        .unwrap();
    assert!(requests
        .windows(2)
        .all(|pair| pair[1].heartbeat.sequence > pair[0].heartbeat.sequence));
    assert!(requests.iter().any(|r| {
        r.heartbeat
            .collector_states
            .iter()
            .any(|s| s.phase == WorkerPhase::Running)
    }));
    assert!(requests.iter().any(
        |r| r.heartbeat.collections.iter().any(|c| c.status == "timeout"
            && (c.collector == "mount.inventory" || c.collector == "iokit.block"))
    ));
    let mut collections = BTreeMap::new();
    let mut repeated_measurement = false;
    for request in requests {
        assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
        assert!(!String::from_utf8_lossy(&request.body).contains(TOKEN));
        for collection in request.heartbeat.collections {
            if let Some(previous) =
                collections.insert(collection.collection_id.clone(), collection.clone())
            {
                assert_eq!(
                    previous, collection,
                    "repeated observations retain content and acquisition timestamps"
                );
                repeated_measurement |= !collection.metrics.is_empty();
            }
        }
    }
    assert!(
        repeated_measurement,
        "unchanged native-independent readings must retain collection IDs between delivered heartbeats"
    );
}
