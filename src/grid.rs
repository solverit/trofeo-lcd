//! The original six-panel dashboard. Landscape uses a 3×2 grid. In portrait
//! the date/time and now-playing panels each span the full width.
use super::dashboard::{
    draw_big_value, draw_metric_cell, draw_panel_chrome, Palette, Readings, Rect,
};
use super::{
    format_freq_mhz, format_uptime, is_portrait, level_color, ColorMode, Framebuffer, Local,
    Marquee, Rotation, System, MARQUEE_GAP,
};

#[derive(Clone, Copy)]
struct Style {
    palette: Palette,
    label: u32,
    value: u32,
}

fn panel(fb: &mut Framebuffer, r: Rect, title: &str, style: Style) -> Rect {
    draw_panel_chrome(fb, r, title, style.palette, style.label)
}

fn metric(fb: &mut Framebuffer, r: Rect, title: &str, value: &str, style: Style) {
    draw_metric_cell(
        fb,
        r,
        title,
        value,
        None,
        style.palette,
        style.value,
        style.label,
    );
}

fn value(fb: &mut Framebuffer, r: Rect, text: &str, style: Style) {
    draw_big_value(fb, r, text, style.palette, style.value);
}

fn layout(w: u32, h: u32, portrait: bool) -> [Rect; 6] {
    let canvas = Rect { x: 0, y: 0, w, h };
    if portrait {
        let area = canvas.inset(5, 7);
        let row = |i| area.row(i, 4);
        let shrink = (row(2).h / 6).min(70);
        let date = Rect {
            h: row(2).h - shrink,
            ..row(2)
        };
        let song = Rect {
            y: row(3).y - shrink,
            h: row(3).h + shrink,
            ..row(3)
        };
        [
            row(0).col(0, 2),
            row(0).col(1, 2),
            row(1).col(0, 2),
            row(1).col(1, 2),
            date,
            song,
        ]
        .map(|r| r.inset(3, 3))
    } else {
        let area = canvas.inset(11, 5);
        std::array::from_fn(|i| area.row(i as u32 / 3, 2).col(i as u32 % 3, 3).inset(5, 5))
    }
}

// The large usage value goes beside its details in landscape and above them
// in portrait. CPU has two full-width rows for frequency and uptime.
fn processor(
    fb: &mut Framebuffer,
    r: Rect,
    usage: &str,
    details: [(&str, String); 4],
    portrait: bool,
    cpu: bool,
    style: Style,
) {
    let (usage_rect, cells) = if portrait && cpu {
        (
            r.row(0, 4),
            [
                r.row(1, 4).col(0, 2),
                r.row(1, 4).col(1, 2),
                r.row(2, 4),
                r.row(3, 4),
            ],
        )
    } else if portrait {
        (
            r.row(0, 3),
            std::array::from_fn(|i| r.row(1 + i as u32 / 2, 3).col(i as u32 % 2, 2)),
        )
    } else {
        (
            r.col(0, 3),
            std::array::from_fn(|i| r.col(1 + i as u32 % 2, 3).row(i as u32 / 2, 2)),
        )
    };
    metric(fb, usage_rect, "USAGE", usage, style);
    for (cell, (label, text)) in cells.into_iter().zip(details) {
        metric(fb, cell, label, &text, style);
    }
}

fn activity(
    fb: &mut Framebuffer,
    r: Rect,
    net: (f64, f64),
    disk: (f64, f64),
    portrait: bool,
    style: Style,
) {
    for (i, (group, labels, values, unit)) in [
        ("NET", ["DL", "UP"], [net.0, net.1], "KB/S"),
        ("DISK", ["READ", "WRITE"], [disk.0, disk.1], "MB/S"),
    ]
    .into_iter()
    .enumerate()
    {
        let group_rect = if portrait {
            r.row(i as u32, 2)
        } else {
            r.col(i as u32, 2)
        };
        let body = panel(fb, group_rect, group, style);
        for j in 0..2 {
            let rect = body.row(j, 2);
            let v = values[j as usize];
            let text = if i == 0 {
                format!("{v:.0} {unit}")
            } else {
                format!("{v:.1} {unit}")
            };
            metric(fb, rect, labels[j as usize], &text, style);
        }
    }
}

fn date_time(
    fb: &mut Framebuffer,
    r: Rect,
    volume: Option<(f32, bool)>,
    portrait: bool,
    style: Style,
) {
    let columns = if portrait { 4 } else { 3 };
    let vol = r.col(0, columns);
    let date = Rect {
        x: r.x + vol.w,
        w: r.w - vol.w,
        ..r
    };
    let volume_text = match volume {
        Some((_, true)) => "MUTE".into(),
        Some((n, false)) => format!("{n:.0}%"),
        None => "N/A".into(),
    };
    metric(fb, vol, "VOL", &volume_text, style);
    if let (true, Some((n, false))) = (portrait, volume) {
        // Tint the panel below the volume level while preserving the text.
        let inner = vol.inset(
            style.palette.border_thickness,
            style.palette.border_thickness,
        );
        let height = (inner.h as f32 * (n / 100.0).clamp(0.0, 1.0)) as u32;
        let (r, g, b) = style.palette.border;
        let bg = style.palette.panel_bg;
        let stride = fb.width() as usize * 3;
        for y in inner.y + inner.h - height..inner.y + inner.h {
            let off = y as usize * stride + inner.x as usize * 3;
            for pixel in fb.as_bytes_mut()[off..off + inner.w as usize * 3].chunks_exact_mut(3) {
                if pixel == [bg.0, bg.1, bg.2] {
                    pixel.copy_from_slice(&[r / 3, g / 3, b / 3]);
                }
            }
        }
    }
    let now = Local::now();
    value(
        fb,
        date.row(0, 2),
        &now.format("%H:%M:%S").to_string(),
        style,
    );
    value(
        fb,
        date.row(1, 2),
        &now.format("%Y-%m-%d").to_string(),
        style,
    );
}

fn now_playing(
    fb: &mut Framebuffer,
    r: Rect,
    title: &str,
    marquee: &mut Marquee,
    bars: &[f32],
    color: ColorMode,
    portrait: bool,
    style: Style,
) {
    let p = style.palette;
    let text_h = if portrait {
        (Framebuffer::text_height(style.label) + 24).min(r.h / 2)
    } else {
        r.h / 2
    };
    let text_rect = Rect { h: text_h, ..r };
    value(fb, text_rect, "", style);
    let clip = text_rect.inset(p.border_thickness + 2, p.border_thickness);
    let scale = style.label.min(clip.h / Framebuffer::text_height(1)).max(1);
    let y = clip.y + clip.h.saturating_sub(Framebuffer::text_height(scale)) / 2;
    let (red, green, blue) = p.value_color;
    if marquee.tick(title, clip.w, scale) {
        let text = format!("{title}{MARQUEE_GAP}");
        let x = clip.x as i64 - marquee.offset_px as i64;
        for start in [x, x + Framebuffer::text_width(&text, scale) as i64] {
            fb.draw_text_clipped(
                start,
                y,
                &text,
                red,
                green,
                blue,
                scale,
                clip.x,
                clip.x + clip.w,
            );
        }
    } else {
        let x = clip.x + clip.w.saturating_sub(Framebuffer::text_width(title, scale)) / 2;
        fb.draw_text(x, y, title, red, green, blue, scale);
    }
    let eq = Rect {
        y: r.y + text_h,
        h: r.h - text_h,
        ..r
    };
    value(fb, eq, "", style);
    let area = eq.inset(p.border_thickness, p.border_thickness);
    for (i, &level) in bars.iter().enumerate() {
        let bar = area.col(i as u32, bars.len() as u32);
        let h = (level * bar.h as f32).round() as u32;
        let (red, green, blue) = level_color(level, color);
        fb.fill_rect(
            bar.x,
            bar.y + bar.h - h,
            bar.w.saturating_sub(1),
            h,
            red,
            green,
            blue,
        );
    }
}

pub(super) fn draw(
    fb: &mut Framebuffer,
    data: &Readings<'_>,
    title: Option<&str>,
    marquee: &mut Marquee,
    color: ColorMode,
    mut palette: Palette,
    rotation: Rotation,
    bars: &[f32],
) {
    let Readings {
        sys,
        gpu_percent,
        gpu,
        cpu_temp,
        cpu_power,
        cpu_mhz,
        net,
        disk,
        volume,
    } = *data;
    let portrait = is_portrait(rotation) && fb.height() > fb.width();
    let rects = layout(fb.width(), fb.height(), portrait);
    let small = rects[0].w < 170 || rects[0].h < 150;
    if small {
        palette.border_thickness = 1;
    }
    let style = Style {
        palette,
        label: if small { 1 } else { 3 },
        value: if small {
            2
        } else if portrait {
            3
        } else {
            5
        },
    };
    let [cpu_box, gpu_box, mem, io, date, song] = std::array::from_fn(|i| {
        panel(
            fb,
            rects[i],
            ["CPU", "GPU", "MEM", "ACTIVITY", "DATE TIME", "NOW PLAYING"][i],
            style,
        )
    });
    let na = || "N/A".to_string();
    processor(
        fb,
        cpu_box,
        &format!("{:.0}%", sys.global_cpu_info().cpu_usage()),
        [
            ("TEMP", cpu_temp.map_or_else(na, |x| format!("{x:.0}C"))),
            ("WATT", cpu_power.map_or_else(na, |x| format!("{x:.0}W"))),
            ("FREQ", cpu_mhz.map_or_else(na, format_freq_mhz)),
            ("UP", format_uptime(System::uptime())),
        ],
        portrait,
        true,
        style,
    );
    processor(
        fb,
        gpu_box,
        &gpu_percent.map_or_else(na, |x| format!("{x:.0}%")),
        [
            ("TEMP", gpu.temp_edge_c.map_or_else(na, |x| format!("{x}C"))),
            ("WATT", gpu.power_w.map_or_else(na, |x| format!("{x}W"))),
            ("RPM", gpu.fan_rpm.map_or_else(na, |x| x.to_string())),
            ("FPS", gpu.fps.map_or_else(na, |x| x.to_string())),
        ],
        portrait,
        false,
        style,
    );
    let first = mem.col(0, 3);
    let rest = Rect {
        x: mem.x + first.w,
        w: mem.w - first.w,
        ..mem
    };
    let cells = if portrait {
        [mem.row(0, 3), mem.row(1, 3), mem.row(2, 3)]
    } else {
        [first, rest.row(0, 2), rest.row(1, 2)]
    };
    let used = sys.used_memory();
    let total = sys.total_memory();
    for (cell, (label, text)) in cells.into_iter().zip([
        (
            "USAGE",
            format!("{:.0}%", 100.0 * used as f64 / total.max(1) as f64),
        ),
        ("USED", format!("{}MB", used / 1048576)),
        ("TOTAL", format!("{}MB", total / 1048576)),
    ]) {
        metric(fb, cell, label, &text, style);
    }
    activity(fb, io, net, disk, portrait, style);
    date_time(fb, date, volume, portrait, style);
    now_playing(
        fb,
        song,
        title.unwrap_or("-"),
        marquee,
        bars,
        color,
        portrait,
        style,
    );
}
