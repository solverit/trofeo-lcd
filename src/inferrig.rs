//! Independent lifecycle and telemetry snapshots from the InferRig API.
use anyhow::{ensure, Result};
use serde::{de::DeserializeOwned, Deserialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default)]
pub struct InferrigState {
    pub lifecycle: Option<Lifecycle>,
    pub telemetry: Telemetry,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Lifecycle {
    pub state: String,
    pub active_model: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct Telemetry {
    pub prompt_tokens_per_second: Option<f64>,
    pub generation_tokens_per_second: Option<f64>,
    pub requests_active: Option<u64>,
    pub requests_queued: Option<u64>,
    pub host: Option<Host>,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Host {
    pub gpu_memory: Option<GpuMemory>,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GpuMemory {
    pub used_bytes: Option<u64>,
    pub kind: Option<String>,
}

pub type SharedState = Arc<Mutex<InferrigState>>;

pub fn base_url(raw: &str) -> Result<String> {
    let raw = raw.trim().trim_end_matches('/');
    let base = if raw.contains("://") {
        raw.to_owned()
    } else {
        let authority: ureq::http::uri::Authority = raw.parse()?;
        let port = if authority.port().is_none() {
            ":5577"
        } else {
            ""
        };
        format!("http://{authority}{port}")
    };
    let uri: ureq::http::Uri = base.parse()?;
    ensure!(
        matches!(uri.scheme_str(), Some("http" | "https"))
            && uri.host().is_some()
            && uri.query().is_none()
            && !base.contains('#'),
        "Invalid InferRig HTTP(S) base URL"
    );
    Ok(base)
}

fn get<T: DeserializeOwned>(agent: &ureq::Agent, url: String) -> Result<T> {
    let mut response = agent.get(url.as_str()).call()?;
    ensure!(
        response.status().is_success(),
        "InferRig returned {}",
        response.status()
    );
    Ok(response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_json()?)
}

fn poll(agent: &ureq::Agent, url: &str) -> InferrigState {
    InferrigState {
        lifecycle: get(agent, format!("{url}/api/state")).ok(),
        telemetry: get(agent, format!("{url}/api/telemetry")).unwrap_or_default(),
    }
}

pub fn spawn(url: String) -> Result<SharedState> {
    let shared = SharedState::default();
    let state = shared.clone();
    std::thread::Builder::new()
        .name("inferrig-poll".into())
        .spawn(move || {
            let agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(3)))
                .build()
                .into();
            loop {
                let fresh = poll(&agent, &url);
                *state.lock().unwrap() = fresh;
                std::thread::sleep(POLL_INTERVAL);
            }
        })?;
    Ok(shared)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_https_and_validates_urls() {
        assert_eq!(base_url("localhost").unwrap(), "http://localhost:5577");
        assert_eq!(
            base_url("localhost:8080/").unwrap(),
            "http://localhost:8080"
        );
        assert_eq!(
            base_url("https://[::1]:8443/base/").unwrap(),
            "https://[::1]:8443/base"
        );
        assert!(base_url("ftp://server").is_err());
    }

    #[test]
    fn telemetry_failure_preserves_lifecycle() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for (path, status, body) in [
                (
                    "state",
                    "200 OK",
                    r#"{"state":"ready","active_model":"модель"}"#,
                ),
                ("telemetry", "503 Unavailable", ""),
            ] {
                let (mut socket, _) = listener.accept().unwrap();
                let mut lines = BufReader::new(&socket).lines().map(|line| line.unwrap());
                assert!(lines
                    .next()
                    .unwrap()
                    .starts_with(&format!("GET /api/{path} ")));
                for line in lines {
                    if line.is_empty() {
                        break;
                    }
                }
                write!(
                    socket,
                    "HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                // A chunk boundary may fall inside a Unicode character.
                for byte in body.as_bytes() {
                    socket.write_all(&[b'1', 13, 10, *byte, 13, 10]).unwrap();
                }
                socket.write_all(b"0\r\n\r\n").unwrap();
            }
        });
        let state = poll(
            &ureq::Agent::config_builder().proxy(None).build().into(),
            &url,
        );
        server.join().unwrap();
        assert_eq!(
            state.lifecycle.unwrap().active_model.as_deref(),
            Some("модель")
        );
        assert!(state.telemetry.requests_active.is_none());
    }
}
