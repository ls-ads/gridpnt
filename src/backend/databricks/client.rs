//! A small HTTPS client for the Databricks REST API: JSON in, JSON out,
//! bearer token attached. Certificates are checked against the operating
//! system's trust store, so corporate root certificates work without any
//! setup, and proxy environment variables are honoured.

use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig};

use super::auth::{Credentials, TokenSource};
use crate::backend::Result;

/// How long one HTTP request may take. Queries are polled, so no single
/// request needs to wait for a slow query.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Largest response body accepted. Inline results are at most 25 MiB.
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

pub struct Client {
    agent: Agent,
    host: String,
    source: TokenSource,
    /// The token in use, fetched on first need.
    token: Mutex<Option<String>>,
}

/// The body Databricks sends with an error status.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ErrorBody {
    message: String,
}

impl Client {
    pub fn new(credentials: Credentials) -> Self {
        let tls = TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build();
        let config = Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(REQUEST_TIMEOUT))
            // Error statuses are read here so their message can be shown.
            .http_status_as_error(false)
            .user_agent(concat!("gridpnt/", env!("CARGO_PKG_VERSION")))
            .build();
        Client {
            agent: Agent::new_with_config(config),
            host: credentials.host,
            source: credentials.source,
            token: Mutex::new(None),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)]) -> Result<T> {
        self.request(path, query, None)
    }

    pub fn post<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> Result<T> {
        self.request(path, &[], Some(body.to_string()))
    }

    fn request<T: DeserializeOwned>(&self, path: &str, query: &[(&str, &str)], body: Option<String>) -> Result<T> {
        let (mut status, mut text) = self.attempt(path, query, body.as_deref())?;

        // A token from the CLI lasts about an hour. If it has been
        // rejected, fetch a fresh one and try once more.
        if status == 401 && self.source.can_refresh() {
            *self.token.lock().unwrap() = None;
            (status, text) = self.attempt(path, query, body.as_deref())?;
        }

        if !(200..300).contains(&status) {
            let detail = serde_json::from_str::<ErrorBody>(&text).unwrap_or_default().message;
            let detail = if detail.is_empty() {
                "no details given".into()
            } else {
                detail
            };
            return Err(format!("Databricks said no (HTTP {status}): {detail}"));
        }
        // Some calls answer with an empty body; treat that as `{}`.
        let text = if text.trim().is_empty() { "{}" } else { &text };
        serde_json::from_str(text).map_err(|e| format!("unexpected reply from Databricks: {e}"))
    }

    /// One HTTP exchange. Returns the status code and the body text.
    fn attempt(&self, path: &str, query: &[(&str, &str)], body: Option<&str>) -> Result<(u16, String)> {
        let token = self.current_token()?;
        let url = format!("{}{path}", self.host);
        let bearer = format!("Bearer {token}");

        let sent = match body {
            None => {
                let mut request = self.agent.get(&url).header("Authorization", &bearer);
                for (key, value) in query {
                    request = request.query(key, value);
                }
                request.call()
            }
            Some(json) => self
                .agent
                .post(&url)
                .header("Authorization", &bearer)
                .content_type("application/json")
                .send(json),
        };
        let mut response = sent.map_err(|e| format!("cannot reach {}: {e}", self.host))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_string()
            .map_err(|e| format!("could not read the reply from Databricks: {e}"))?;
        Ok((status, text))
    }

    fn current_token(&self) -> Result<String> {
        let mut token = self.token.lock().unwrap();
        if token.is_none() {
            *token = Some(self.source.token()?);
        }
        Ok(token.clone().unwrap_or_default())
    }
}

/// Escape text for use as one segment of a URL path.
pub fn path_segment(text: &str) -> String {
    let mut escaped = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            escaped.push(byte as char);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}
