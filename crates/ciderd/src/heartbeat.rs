//! One bounded HTTP attempt over a caller-provided immutable snapshot.
use crate::{
    config::HeartbeatConfig,
    model::{parse_json, Acknowledgement, Heartbeat},
};
use anyhow::{ensure, Context, Result};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER},
    Client,
};
use std::{
    fmt,
    time::{Duration, SystemTime},
};

pub struct Sender {
    config: HeartbeatConfig,
    client: Client,
    endpoint: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    Network,
    Authentication,
    Contract,
    PayloadTooLarge,
    RateLimited,
    Server,
    InvalidAcknowledgement,
    ResponseTooLarge,
}
#[derive(Debug)]
pub struct SendFailure {
    pub kind: FailureKind,
    pub retry_after: Option<Duration>,
}
impl fmt::Display for SendFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "heartbeat {:?}", self.kind)
    }
}
impl std::error::Error for SendFailure {}
impl From<FailureKind> for SendFailure {
    fn from(kind: FailureKind) -> Self {
        Self {
            kind,
            retry_after: None,
        }
    }
}

impl Sender {
    pub fn new(config: HeartbeatConfig) -> Result<Self> {
        // Config is public so callers may construct it without Config::parse.
        let url = reqwest::Url::parse(&config.endpoint)?;
        ensure!(
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
                && url.query().is_none(),
            "heartbeat requires a credential-free HTTPS endpoint"
        );
        ensure!(
            !config.follow_redirects && config.delivery_mode == "latest",
            "unsupported transport policy"
        );
        ensure!(
            config.connect_timeout_seconds > 0 && config.request_timeout_seconds > 0,
            "HTTP deadlines must be positive"
        );
        let mut client = Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(config.connect_timeout_seconds))
            .timeout(Duration::from_secs(config.request_timeout_seconds))
            .user_agent(concat!("ciderd/", env!("CARGO_PKG_VERSION")));
        if let Some(path) = &config.ca_certificate_file {
            ensure!(path.is_absolute(), "CA certificate path must be absolute");
            let metadata =
                std::fs::symlink_metadata(path).context("cannot inspect CA certificate file")?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "CA certificate must be a regular file"
            );
            let pem = crate::config::read_bounded(path, 65536)
                .context("cannot read CA certificate file")?;
            let certificates = reqwest::Certificate::from_pem_bundle(&pem)
                .context("invalid CA certificate PEM")?;
            ensure!(
                !certificates.is_empty() && certificates.len() <= 64,
                "CA certificate file must contain 1..64 certificates"
            );
            for certificate in certificates {
                client = client.add_root_certificate(certificate);
            }
        }
        let client = client.build()?;
        Ok(Self {
            endpoint: config.endpoint.clone(),
            config,
            client,
        })
    }

    pub async fn send(
        &self,
        heartbeat: &Heartbeat,
        authorization: &HeaderValue,
    ) -> std::result::Result<Acknowledgement, SendFailure> {
        heartbeat
            .validate()
            .map_err(|_| SendFailure::from(FailureKind::Contract))?;
        let body =
            serde_json::to_vec(heartbeat).map_err(|_| SendFailure::from(FailureKind::Contract))?;
        if body.len() > self.config.maximum_request_bytes {
            return Err(FailureKind::PayloadTooLarge.into());
        }
        let mut response = self
            .client
            .post(&self.endpoint)
            .header(AUTHORIZATION, authorization.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|_| SendFailure::from(FailureKind::Network))?;
        let status = response.status().as_u16();
        if status != 200 {
            let kind = match status {
                401 | 403 => FailureKind::Authentication,
                400 | 422 => FailureKind::Contract,
                413 => FailureKind::PayloadTooLarge,
                429 => FailureKind::RateLimited,
                500..=599 => FailureKind::Server,
                _ => FailureKind::Contract,
            };
            return Err(SendFailure {
                kind,
                retry_after: if status == 429 {
                    response
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(retry_after)
                } else {
                    None
                },
            });
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.config.maximum_response_bytes as u64)
        {
            return Err(FailureKind::ResponseTooLarge.into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| SendFailure::from(FailureKind::Network))?
        {
            if chunk.len()
                > self
                    .config
                    .maximum_response_bytes
                    .saturating_sub(bytes.len())
            {
                return Err(FailureKind::ResponseTooLarge.into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let ack: Acknowledgement = parse_json(&bytes)
            .map_err(|_| SendFailure::from(FailureKind::InvalidAcknowledgement))?;
        ack.validate_for(heartbeat)
            .map_err(|_| SendFailure::from(FailureKind::InvalidAcknowledgement))?;
        Ok(ack)
    }
}

pub fn bearer_header(token: &str) -> Result<HeaderValue> {
    ensure!(
        !token.is_empty() && token.len() <= 4096 && token.bytes().all(|b| b.is_ascii_graphic()),
        "credential must contain 1..4096 printable ASCII bytes without whitespace"
    );
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))?;
    value.set_sensitive(true);
    Ok(value)
}

fn retry_after(value: &str) -> Option<Duration> {
    if let Ok(n) = value.parse::<u64>() {
        return Some(Duration::from_secs(n));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let when: SystemTime = date.into();
    Some(when.duration_since(SystemTime::now()).unwrap_or_default())
}

pub struct RetryPolicy {
    interval: Duration,
    maximum: Duration,
    failures: u32,
}
impl RetryPolicy {
    pub fn new(interval: Duration, maximum: Duration) -> Self {
        Self {
            interval,
            maximum,
            failures: 0,
        }
    }
    pub fn succeeded(&mut self) {
        self.failures = 0;
    }
    pub fn failed(&mut self, retry_after: Option<Duration>) -> Duration {
        self.failures = self.failures.saturating_add(1);
        if let Some(delay) = retry_after {
            return delay.max(self.interval).min(self.maximum);
        }
        let base = self.interval.saturating_mul(1u32 << self.failures.min(16));
        let random = uuid::Uuid::new_v4().as_bytes()[0] as u32;
        let millis = base.as_millis().saturating_mul(80 + random as u128 % 21) / 100;
        Duration::from_millis(millis.min(u64::MAX as u128) as u64)
            .max(self.interval)
            .min(self.maximum)
    }
}

#[cfg(test)]
mod tests;
