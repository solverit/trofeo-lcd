//! Compact LCD view of the inference host's /api/status-board snapshot.
use super::{
    dashboard::Rect, fit_text_scale, format_uptime, inferrig, ColorMode, Framebuffer, Marquee,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Board {
    profile: Option<NamedProfile>,
    lifecycle: Lifecycle,
    telemetry: Telemetry,
}

#[derive(Deserialize)]
struct NamedProfile {
    name: String,
}

#[derive(Default, Deserialize)]
struct Lifecycle {
    active_profile: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Telemetry {
    #[serde(flatten)]
    current: Sample,
    history: Vec<Sample>,
    run_started_at: Option<DateTime<Utc>>,
    #[serde(flatten)]
    context: Context,
    usage: Context,
    error: String,
}

#[derive(Default, Deserialize)]
struct Context {
    context_current: Option<u64>,
    context_configured: Option<u64>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Sample {
    sampled_at: Option<DateTime<Utc>>,
    generation_tokens_per_second: Option<f64>,
    host: Host,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Host {
    gpu: Processor,
    cpu: Processor,
    gpu_memory: Memory,
    memory: Memory,
    apu_power_watts: Option<f64>,
    network_rx_bytes_per_second: Option<f64>,
    network_tx_bytes_per_second: Option<f64>,
}

#[derive(Default, Deserialize)]
struct Processor {
    utilization_percent: Option<f64>,
    temperature_c: Option<f64>,
}

#[derive(Default, Deserialize)]
struct Memory {
    used_bytes: Option<u64>,
    total_bytes: Option<u64>,
    budget_bytes: Option<u64>,
    kind: Option<String>,
}

pub(super) fn poll(agent: &ureq::Agent, url: &str) -> Option<Arc<Board>> {
    inferrig::get(agent, format!("{url}/api/status-board"))
        .ok()
        .map(Arc::new)
}

#[derive(Clone, Copy, PartialEq)]
enum Metric {
    Speed,
    Gpu,
    VideoMemory,
    Temp,
    Power,
    Cpu,
    Ram,
    Net,
}
use Metric::*;
const METRICS: [Metric; 8] = [Speed, Gpu, VideoMemory, Temp, Power, Cpu, Ram, Net];
const GIB: f64 = 1073741824.0;

impl Metric {
    fn label(self, host: &Host) -> &'static str {
        match self {
            Speed => "GEN SPEED",
            Gpu => "GPU LOAD",
            VideoMemory => match host.gpu_memory.kind.as_deref() {
                Some("gtt") => "GPU MEM / GTT",
                Some("vram") => "VRAM",
                _ => "GPU MEMORY",
            },
            Temp => "GPU TEMP",
            Power => "APU POWER",
            Cpu => "CPU LOAD",
            Ram => "SYSTEM RAM",
            Net => "NET RX+TX",
        }
    }

    fn value(self, sample: &Sample) -> Option<f64> {
        let host = &sample.host;
        match self {
            Speed => sample.generation_tokens_per_second,
            Gpu => host.gpu.utilization_percent,
            VideoMemory => host.gpu_memory.used_bytes.map(|n| n as f64 / GIB),
            Temp => host.gpu.temperature_c,
            Power => host.apu_power_watts,
            Cpu => host.cpu.utilization_percent,
            Ram => host.memory.used_bytes.map(|n| n as f64 / GIB),
            Net => Some(host.network_rx_bytes_per_second? + host.network_tx_bytes_per_second?),
        }
    }

    fn ceiling(self, host: &Host) -> Option<f64> {
        match self {
            Gpu | Cpu | Temp => Some(100.0),
            VideoMemory => host.gpu_memory.budget_bytes.map(|n| n as f64 / GIB),
            Ram => host.memory.total_bytes.map(|n| n as f64 / GIB),
            _ => None,
        }
        .filter(|n| *n > 0.0)
    }

    fn formatted(self, v: Option<f64>, host: &Host) -> (String, String) {
        let number = |n: Option<f64>, precision| {
            n.map_or_else(|| "--".into(), |x| format!("{x:.precision$}"))
        };
        match self {
            Speed => (number(v, 1), "tok/s".into()),
            Cpu | Gpu => (number(v, 0), "%".into()),
            Temp => (number(v, 0), "°C".into()),
            Power => (number(v, 0), "W".into()),
            VideoMemory | Ram => (
                number(v, 1),
                self.ceiling(host)
                    .map_or_else(|| "GiB".into(), |n| format!("/ {n:.0} GiB")),
            ),
            Net => {
                let (div, unit) = if v.unwrap_or(0.0) >= 1048576.0 {
                    (1048576.0, "MiB/s")
                } else {
                    (1024.0, "KiB/s")
                };
                (number(v.map(|n| n / div), 1), unit.into())
            }
        }
    }
}

type Rgb = (u8, u8, u8);
const BG: Rgb = (24, 31, 33);
const SURFACE: Rgb = (29, 37, 39);
const EDGE: Rgb = (92, 109, 109);
const MUTED: Rgb = (175, 193, 191);
const WHITE: Rgb = (235, 243, 240);
const MINT: Rgb = (65, 212, 161);
const AMBER: Rgb = (242, 188, 103);

fn fill(fb: &mut Framebuffer, r: Rect, c: Rgb) {
    fb.fill_rect(r.x, r.y, r.w, r.h, c.0, c.1, c.2);
}

fn text(fb: &mut Framebuffer, x: u32, y: u32, s: &str, scale: u32, c: Rgb) {
    fb.draw_text(x, y, s, c.0, c.1, c.2, scale);
}

fn tokens(value: Option<u64>) -> String {
    match value {
        Some(n) if n >= 1024 => {
            let k = format!("{:.1}", n as f64 / 1024.0);
            format!("{}K", k.trim_end_matches(".0"))
        }
        Some(n) => n.to_string(),
        None => "--".into(),
    }
}

fn header(
    fb: &mut Framebuffer,
    board: Option<&Board>,
    now: DateTime<Utc>,
    marquee: &mut Marquee,
    accent: Rgb,
    r: Rect,
) {
    let t = board.map(|b| &b.telemetry);
    let name = board.map_or("INFERRIG OFFLINE", |b| {
        b.profile
            .as_ref()
            .map(|p| p.name.as_str())
            .filter(|s| !s.is_empty())
            .or(b.lifecycle.active_profile.as_deref())
            .unwrap_or("NO ACTIVE PROFILE")
    });
    let uptime = t.and_then(|t| t.run_started_at).map_or_else(
        || "--".into(),
        |start| format_uptime((now - start).num_seconds().max(0) as u64),
    );
    let uptime = format!("UP {uptime}");
    let detail = (r.h / 20).clamp(1, 3).min((r.w / 210).max(1));
    let up_width = Framebuffer::text_width(&uptime, detail);
    let stacked = r.w < 280;
    let right = r.x + r.w;
    let name_width = if stacked {
        r.w
    } else {
        r.w.saturating_sub(up_width + 12)
    };
    let title_scale = (r.h / 16).clamp(1, 5).min((r.w / 130).max(1));
    let top = r.y;
    marquee.draw(
        fb,
        name,
        Rect { w: name_width, ..r },
        title_scale,
        WHITE,
        false,
    );
    let up_y = if stacked {
        top + Framebuffer::text_height(title_scale) + 6
    } else {
        top + Framebuffer::text_height(title_scale) - Framebuffer::text_height(detail)
    };
    text(
        fb,
        if stacked { r.x } else { right - up_width },
        up_y,
        &uptime,
        detail,
        MUTED,
    );

    let t = t.filter(|t| t.error.is_empty());
    let used = t.and_then(|t| t.context.context_current.or(t.usage.context_current));
    let total = t.and_then(|t| t.context.context_configured.or(t.usage.context_configured));
    let ratio = used
        .zip(total)
        .filter(|(_, n)| *n > 0)
        .map(|(u, n)| u as f64 / n as f64);
    let percent = ratio.map_or(String::new(), |v| format!("  {:.0}%", v * 100.0));
    let context = format!("CTX {} / {}{}", tokens(used), tokens(total), percent);
    let scale = fit_text_scale(&context, r.w, detail);
    let y = top + r.h - Framebuffer::text_height(scale);
    text(fb, r.x, y, &context, scale, accent);
    let bar_x = r.x + Framebuffer::text_width(&context, scale) + 6 * scale;
    let bar_w = right.saturating_sub(bar_x);
    if bar_w >= 24 {
        let track = Rect {
            x: bar_x,
            y: y + scale,
            w: bar_w,
            h: 5 * scale,
        };
        fill(fb, track, SURFACE);
        if let Some(v) = ratio {
            fill(
                fb,
                Rect {
                    w: (v.clamp(0.0, 1.0) * bar_w as f64) as u32,
                    ..track
                },
                accent,
            );
        }
        for x in (0..bar_w).step_by((6 * scale) as usize) {
            fill(
                fb,
                Rect {
                    x: bar_x + x,
                    w: 1,
                    ..track
                },
                BG,
            );
        }
    }
}

// One column per five seconds of server samples. Missing readings leave gaps.
const BINS: usize = 12;
fn history(t: &Telemetry, metric: Metric) -> [Option<f64>; BINS] {
    let mut bins = [None; BINS];
    let Some(end) = t.current.sampled_at else {
        return bins;
    };
    for sample in &t.history {
        let Some(at) = sample.sampled_at else {
            continue;
        };
        let age = (end - at).num_milliseconds();
        if (0..60_000).contains(&age) {
            bins[BINS - 1 - age as usize / 5000] = metric.value(sample);
        }
    }
    bins[BINS - 1] = if metric == Speed && !t.error.is_empty() {
        None
    } else {
        metric.value(&t.current)
    };
    bins
}

fn card(fb: &mut Framebuffer, r: Rect, t: &Telemetry, metric: Metric, accent: Rgb) {
    let host = &t.current.host;
    let label = metric.label(host);
    let pad = (r.w / 22).clamp(4, 18);
    let inner = r.inset(pad, 0);
    let cap = (r.h / 44).clamp(1, 4);
    let label_scale = fit_text_scale(label, inner.w, cap);
    let label_h = Framebuffer::text_height(label_scale);
    let body = Rect {
        y: r.y + label_h / 2,
        h: r.h - label_h / 2,
        ..r
    };
    fill(fb, body, EDGE);
    fill(fb, body.inset(1, 1), SURFACE);
    let label_w = Framebuffer::text_width(label, label_scale);
    fill(
        fb,
        Rect {
            x: inner.x - 2,
            y: r.y,
            w: label_w + 4,
            h: label_h,
        },
        BG,
    );
    text(fb, inner.x, r.y, label, label_scale, MUTED);

    let value = metric
        .value(&t.current)
        .filter(|_| metric != Speed || t.error.is_empty());
    let (main, unit) = metric.formatted(value, host);
    let small = fit_text_scale(&unit, inner.w / 2, cap);
    let unit_w = Framebuffer::text_width(&unit, small);
    let large = fit_text_scale(
        &main,
        inner.w.saturating_sub(unit_w + 4),
        (r.h / 23).clamp(1, 7),
    );
    let graph_h = (r.h / 3).min(55);
    let bottom = r.y + r.h - (r.h / 13).clamp(3, 12);
    let graph_y = bottom - graph_h;
    let above = r.y + label_h;
    let number_h = Framebuffer::text_height(large);
    let y = above + graph_y.saturating_sub(above + number_h) / 2;
    text(fb, inner.x, y, &main, large, WHITE);
    text(
        fb,
        inner.x + Framebuffer::text_width(&main, large) + 4,
        y + number_h.saturating_sub(Framebuffer::text_height(small)),
        &unit,
        small,
        MUTED,
    );

    let bins = history(t, metric);
    let max = metric
        .ceiling(host)
        .unwrap_or_else(|| bins.iter().flatten().copied().fold(1.0, f64::max));
    let chart = Rect {
        x: inner.x,
        y: graph_y,
        w: inner.w,
        h: graph_h,
    };
    fill(
        fb,
        Rect {
            y: bottom - 1,
            h: 1,
            ..chart
        },
        EDGE,
    );
    for (i, sample) in bins.iter().enumerate() {
        if let Some(v) = sample {
            let col = chart.col(i as u32, BINS as u32);
            let h = ((v / max).clamp(0.0, 1.0) * graph_h as f64).round() as u32;
            let h = h.max(1);
            fill(
                fb,
                Rect {
                    y: bottom - h,
                    h,
                    w: col.w.saturating_sub(2),
                    ..col
                },
                if metric == Temp { AMBER } else { accent },
            );
        }
    }
}

pub(super) fn draw(
    fb: &mut Framebuffer,
    board: Option<&Board>,
    now: DateTime<Utc>,
    marquee: &mut Marquee,
    color: ColorMode,
) {
    let accent = match color {
        ColorMode::Default => MINT,
        ColorMode::Custom(r, g, b) => (r, g, b),
    };
    fb.clear(BG.0, BG.1, BG.2);
    let (w, h) = (fb.width(), fb.height());
    let margin = (w.min(h) / 27).clamp(4, 18);
    let header_h = if w < 280 { 60 } else { (h / 7).clamp(24, 75) };
    header(
        fb,
        board,
        now,
        marquee,
        accent,
        Rect {
            x: margin,
            y: margin,
            w: w - 2 * margin,
            h: header_h,
        },
    );
    let cols = if w < 280 {
        1
    } else if w <= h {
        2
    } else {
        4
    };
    let y = margin + header_h + margin / 2;
    let area = Rect {
        x: margin / 2,
        y,
        w: w - margin,
        h: h - y - margin / 2,
    };
    let empty = Telemetry::default();
    let t = board.map_or(&empty, |b| &b.telemetry);
    for (i, metric) in METRICS.iter().copied().enumerate() {
        let tile = area
            .row(i as u32 / cols, 8 / cols)
            .col(i as u32 % cols, cols)
            .inset(margin / 2, margin / 2);
        card(fb, tile, t, metric, accent);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{png_save, Resolution};
    use super::*;
    use serde_json::json;

    #[test]
    fn status_board_metrics_and_frames() {
        let now: DateTime<Utc> = "2026-01-02T03:04:05Z".parse().unwrap();
        let speeds = [
            13.5, 28.1, 38.2, 33.4, 41.9, 44.6, 48.8, 41.3, 32.6, 38.2, 42.8, 45.1,
        ];
        let samples: Vec<_> = speeds.iter().enumerate().map(|(i, speed)| {
            let at = now - chrono::Duration::seconds(5 * (11 - i as i64));
            json!({
                "sampled_at": at,
                "generation_tokens_per_second": speed,
                "host": {
                    "gpu": {"utilization_percent": ([55,64,78,79,75,84,89,82,94,91,95,96][i]), "temperature_c": 48+i},
                    "gpu_memory": {"kind":"vram","used_bytes": ((8.2+i as f64/5.0)*GIB) as u64, "budget_bytes": 12_u64 << 30},
                    "apu_power_watts": ([64,73,71,86,84,91,102,107,94,103,112,116][i]),
                    "cpu": {"utilization_percent": ([23,34,32,49,56,41,38,51,43,59,46,52][i])},
                    "memory": {"used_bytes": ((30.8 + i as f64/10.0)*GIB) as u64, "total_bytes": 64_u64<<30},
                    "network_rx_bytes_per_second": ([3,5,7,2,1,7,3,8,2,3,6,9][i] << 20),
                    "network_tx_bytes_per_second": 1<<19
                }
            })
        }).collect();
        let mut telemetry = samples.last().unwrap().clone();
        telemetry["history"] = json!(samples);
        telemetry["run_started_at"] = json!(now - chrono::Duration::seconds(8717));
        telemetry["context_current"] = json!(10240);
        telemetry["context_configured"] = json!(32768);
        let board: Board = serde_json::from_value(json!({
            "profile": {"id":"profile-1","name":"Qwen3 Coder"},
            "lifecycle": {"active_profile":"profile-1","active_model":"different-model"},
            "telemetry":telemetry
        }))
        .unwrap();
        let t = &board.telemetry;
        assert_eq!(t.context.context_current, Some(10240));
        assert_eq!(t.current.generation_tokens_per_second, Some(45.1));
        assert_eq!(history(t, Speed), speeds.map(Some));
        assert_eq!(Net.value(&t.current), Some(9.5 * 1048576.0));
        assert_eq!(VideoMemory.ceiling(&t.current.host), Some(12.0));

        let dir = std::path::Path::new("target/dashboard-previews");
        std::fs::create_dir_all(dir).unwrap();
        for (w, h) in [
            (1280, 480),
            (480, 1280),
            (1920, 462),
            (640, 172),
            (172, 640),
            (360, 360),
        ] {
            let mut fb = Framebuffer::new(Resolution::new(w, h));
            for state in [None, Some(&board)] {
                draw(&mut fb, state, now, &mut Marquee::new(), ColorMode::Default);
                let has_bar = fb
                    .as_bytes()
                    .chunks_exact(3)
                    .any(|p| p == [AMBER.0, AMBER.1, AMBER.2]);
                assert_eq!(has_bar, state.is_some());
            }
            std::fs::write(
                dir.join(format!("monitor-{w}x{h}.png")),
                png_save::encode(&fb),
            )
            .unwrap();
        }
    }

    #[test]
    fn missing_history_and_partial_measurements_stay_empty() {
        let t: Telemetry = serde_json::from_value(json!({
            "sampled_at":"2026-01-01T12:01:00Z",
            "generation_tokens_per_second":99,
            "error":"backend unavailable",
            "host":{"network_rx_bytes_per_second":1024},
            "history":[
                {"sampled_at":"2026-01-01T11:59:55Z","generation_tokens_per_second":1},
                {"sampled_at":"2026-01-01T12:00:40Z","generation_tokens_per_second":42}
            ]
        }))
        .unwrap();
        let mut expected = [None; BINS];
        expected[7] = Some(42.0);
        assert_eq!(history(&t, Speed), expected);
        assert_eq!(Net.value(&t.current), None);
    }
}
