//! Integrasi dengan **InferRig** (https://github.com/…/inferrig — control
//! plane lokal untuk llama.cpp di Podman): polling HTTP API-nya secara
//! berkala dari thread background, lalu hasilnya ditampilkan sebagai panel
//! "AI INFERENCE" di layar LCD.
//!
//! Endpoint yang dipakai (default `http://127.0.0.1:5577`):
//! - `GET /api/state` — status lifecycle + nama model/profil aktif.
//! - `GET /api/telemetry` — snapshot telemetry llama-server (tok/s, queue,
//!   context, memori GPU/GTT) hasil agregasi `/metrics` + `/slots`.
//!
//! HTTP client minimal (TcpStream + HTTP/1.1 `Connection: close`) — tidak
//! perlu dependensi async/HTTP berat untuk request localhost sesederhana
//! ini. Parse JSON pakai `serde_json`.
//!
//! InferRig juga bisa JALAN BELAKANGAN: kalau gagal connect, state terakhir
//! ditandai `error` dan panel menampilkan OFFLINE — tidak ada yang crash.

use std::io::{Read, Write as IoWrite};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// Interval default polling API InferRig.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Snapshot data InferRig untuk satu frame — di-clone dari thread polling
/// oleh loop utama tiap frame (murah: struct kecil).
#[derive(Debug, Clone, Default)]
pub struct InferrigState {
    /// Lifecycle state: "ready" / "idle" / "switching" / "error" / "stopped".
    pub state: String,
    /// Nama model aktif (mis. "unsloth-ud-iq4-xs-qwen3-8-…").
    pub model: String,
    /// Nama profil aktif.
    pub profile: String,
    /// Kecepatan decode/generasi, token per detik.
    pub gen_tps: f64,
    /// Kecepatan prefill/prompt, token per detik.
    pub prompt_tps: f64,
    /// Request yang sedang diproses / antre.
    pub requests_active: u64,
    pub requests_queued: u64,
    /// Context terpakai (puncak) vs terkonfigurasi, token.
    pub context_peak: u64,
    pub context_configured: u64,
    /// Memori GPU (GTT) terpakai / budget, byte (dari host telemetry).
    pub gpu_mem_used_bytes: f64,
    pub gpu_mem_budget_bytes: f64,
    /// Usia snapshot terakhir yang BERHASIL — kalau sudah lama sekali
    /// (mis. lebih dari 3x interval), panel menampilkan OFFLINE.
    pub updated: Option<Instant>,
    /// Pesan error terakhir (connect gagal dsb.).
    pub error: Option<String>,
}

impl InferrigState {
    pub fn online(&self) -> bool {
        self.updated.is_some_and(|t| t.elapsed() < Duration::from_secs(10))
    }
}

pub type SharedState = Arc<Mutex<InferrigState>>;

/// Mulai thread polling. Mengembalikan handle state bersama — selalu ada
/// isinya (default = offline) walau thread belum pernah berhasil connect.
pub fn spawn(base_url: String, interval: Duration) -> SharedState {
    let shared: SharedState = Arc::new(Mutex::new(InferrigState::default()));
    let state = Arc::clone(&shared);

    std::thread::Builder::new()
        .name("inferrig-poll".to_string())
        .spawn(move || {
            let url = normalize_url(&base_url);
            loop {
                let fresh = poll_once(&url);
                {
                    let mut guard = state.lock().expect("inferrig state mutex poisoned");
                    match fresh {
                        Ok(mut s) => {
                            s.updated = Some(Instant::now());
                            *guard = s;
                        }
                        Err(e) => {
                            guard.error = Some(e.to_string());
                            guard.updated = None;
                        }
                    }
                }
                std::thread::sleep(interval);
            }
        })
        .expect("gagal spawn thread inferrig-poll");

    shared
}

/// Ambil snapshot terbaru (clone murah) untuk digambar ke frame.
pub fn snapshot(shared: &SharedState) -> InferrigState {
    shared.lock().expect("inferrig state mutex poisoned").clone()
}

// ── respons API (hanya field yang dipakai; sisanya diabaikan) ────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct StateResponse {
    state: Option<String>,
    active_model: Option<String>,
    active_profile: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TelemetryResponse {
    prompt_tokens_per_second: Option<f64>,
    generation_tokens_per_second: Option<f64>,
    requests_active: Option<u64>,
    requests_queued: Option<u64>,
    context_peak: Option<u64>,
    context_configured: Option<u64>,
    host: Option<HostPart>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct HostPart {
    gpu_memory: Option<GpuMemPart>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GpuMemPart {
    used_bytes: Option<f64>,
    budget_bytes: Option<f64>,
}

fn poll_once(base_url: &str) -> anyhow::Result<InferrigState> {
    let mut state = InferrigState::default();

    let state_resp: StateResponse = serde_json::from_str(&http_get(base_url, "/api/state")?)?;
    state.state = state_resp.state.unwrap_or_default();
    state.model = state_resp.active_model.unwrap_or_default();
    state.profile = state_resp.active_profile.unwrap_or_default();

    if let Ok(t) = serde_json::from_str::<TelemetryResponse>(&http_get(base_url, "/api/telemetry")?) {
        state.gen_tps = t.generation_tokens_per_second.unwrap_or(0.0);
        state.prompt_tps = t.prompt_tokens_per_second.unwrap_or(0.0);
        state.requests_active = t.requests_active.unwrap_or(0);
        state.requests_queued = t.requests_queued.unwrap_or(0);
        state.context_peak = t.context_peak.unwrap_or(0);
        state.context_configured = t.context_configured.unwrap_or(0);
        if let Some(gpu_mem) = t.host.and_then(|h| h.gpu_memory) {
            state.gpu_mem_used_bytes = gpu_mem.used_bytes.unwrap_or(0.0);
            state.gpu_mem_budget_bytes = gpu_mem.budget_bytes.unwrap_or(0.0);
        }
    }

    Ok(state)
}

/// GET HTTP/1.1 sederhana via TcpStream (`Connection: close`). Mendukung
/// body dengan Content-Length maupun chunked (sampai koneksi tertutup).
fn http_get(base_url: &str, path: &str) -> anyhow::Result<String> {
    let (host, port) = split_host_port(base_url)?;
    let addr = format!("{host}:{port}");

    let mut stream = TcpStream::connect(&addr)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes())?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;

    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("respons HTTP tanpa pemisah header/body"))?;
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        let status = head.lines().next().unwrap_or("?");
        anyhow::bail!("HTTP gagal: {status}");
    }
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        return Ok(dechunk(body));
    }
    Ok(body.to_string())
}

/// Gabung ulang body HTTP chunked menjadi utuh.
fn dechunk(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    loop {
        let Some((size_line, remainder)) = rest.split_once("\r\n") else {
            break;
        };
        let Ok(size) = usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16)
        else {
            break;
        };
        if size == 0 {
            break;
        }
        let bytes: Vec<u8> = remainder.bytes().take(size).collect();
        out.push_str(&String::from_utf8_lossy(&bytes));
        // Lewati CRLF setelah chunk.
        rest = &remainder[size.min(remainder.len())..];
        rest = rest.strip_prefix("\r\n").unwrap_or(rest);
    }
    out
}

/// "http://127.0.0.1:5577" | "127.0.0.1:5577" | "127.0.0.1" → (host, port).
fn normalize_url(url: &str) -> String {
    let no_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    no_scheme.trim_end_matches('/').to_string()
}

fn split_host_port(base_url: &str) -> anyhow::Result<(String, u16)> {
    let no_scheme = normalize_url(base_url);
    match no_scheme.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => {
            Ok((host.to_string(), port.parse()?))
        }
        _ => Ok((no_scheme, 5577)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_host_port_variants() {
        assert_eq!(
            split_host_port("http://127.0.0.1:5577").unwrap(),
            ("127.0.0.1".to_string(), 5577)
        );
        assert_eq!(
            split_host_port("127.0.0.1:5577/").unwrap(),
            ("127.0.0.1".to_string(), 5577)
        );
        assert_eq!(
            split_host_port("localhost").unwrap(),
            ("localhost".to_string(), 5577)
        );
    }

    #[test]
    fn dechunk_simple() {
        let body = "4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        assert_eq!(dechunk(body), "Wikipedia");
    }

    #[test]
    fn parse_state_and_telemetry_samples() {
        let s: StateResponse = serde_json::from_str(
            r#"{"state":"ready","active_model":"qwen3-8b","active_profile":"p1"}"#,
        )
        .unwrap();
        assert_eq!(s.state.as_deref(), Some("ready"));
        assert_eq!(s.active_model.as_deref(), Some("qwen3-8b"));

        let t: TelemetryResponse = serde_json::from_str(
            r#"{"prompt_tokens_per_second":123.5,"generation_tokens_per_second":42,
                "requests_active":2,"requests_queued":1,"context_peak":900,
                "context_configured":87552,
                "host":{"gpu_memory":{"used_bytes":68.7e9,"budget_bytes":114e9}}}"#,
        )
        .unwrap();
        assert_eq!(t.generation_tokens_per_second, Some(42.0));
        assert_eq!(t.requests_active, Some(2));
        let gm = t.host.unwrap().gpu_memory.unwrap();
        assert!((gm.used_bytes.unwrap() - 68.7e9).abs() < 1.0);
    }
}
