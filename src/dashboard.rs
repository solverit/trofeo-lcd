use super::{
    fit_text_scale, gpu_amd, inferrig, is_portrait, ColorMode, Framebuffer, Marquee, Rotation,
    System, Theme, MARQUEE_GAP,
};

pub(super) struct Readings<'a> {
    pub(super) sys: &'a System,
    pub(super) gpu: &'a gpu_amd::GpuAmdData,
    pub(super) gpu_percent: Option<f32>,
    pub(super) cpu_temp: Option<f32>,
    pub(super) cpu_power: Option<f32>,
    pub(super) cpu_mhz: Option<u32>,
    pub(super) net: (f64, f64),
    pub(super) disk: (f64, f64),
    pub(super) volume: Option<(f32, bool)>,
}

/// Palet warna bersama semua panel dashboard (border, latar, label, nilai,
/// ketebalan bingkai).
#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub(super) border: (u8, u8, u8),
    pub(super) panel_bg: (u8, u8, u8),
    pub(super) label_color: (u8, u8, u8),
    pub(super) value_color: (u8, u8, u8),
    pub(super) border_thickness: u32,
}

impl Palette {
    pub(super) fn new(theme: Theme, color: ColorMode) -> Self {
        let (border, value_color) = match theme {
            Theme::Standard => ((0xE0, 0xE0, 0xE0), (0xF0, 0xF0, 0xF0)),
            Theme::Terminal => ((0x06, 0x3C, 0x1C), (0x00, 0xE0, 0x64)),
        };
        Self {
            border: match color {
                ColorMode::Custom(r, g, b) => (r, g, b),
                _ => border,
            },
            panel_bg: (0x14, 0x14, 0x1C),
            label_color: (0xA0, 0xA0, 0xA8),
            value_color,
            border_thickness: 4,
        }
    }

    pub(super) fn clock(self, color: ColorMode) -> (u8, u8, u8) {
        match color {
            ColorMode::Custom(r, g, b) => (r, g, b),
            _ => self.value_color,
        }
    }
}

fn fill_panel(fb: &mut Framebuffer, rect: Rect, p: Palette) {
    let Rect { x, y, w, h } = rect;
    let bt = p.border_thickness;
    fb.fill_rect(x, y, w, h, p.border.0, p.border.1, p.border.2);
    fb.fill_rect(
        x + bt,
        y + bt,
        w.saturating_sub(bt * 2),
        h.saturating_sub(bt * 2),
        p.panel_bg.0,
        p.panel_bg.1,
        p.panel_bg.2,
    );
}

/// Bingkai panel standar: border luar, latar gelap, label kecil di tengah
/// atas, garis pemisah di bawah label. Mengembalikan area dalam panel
/// `(x, y, w, h)` — MULAI dari garis pemisah (cell yang digambar di area
/// ini sebaiknya menggambar border atasnya sendiri supaya menyatu).
pub(super) fn draw_panel_chrome(
    fb: &mut Framebuffer,
    rect: Rect,
    label: &str,
    p: Palette,
    label_scale: u32,
) -> Rect {
    let Rect { x, y, w, h } = rect;
    let bt = p.border_thickness;
    fill_panel(fb, rect, p);

    let lw = Framebuffer::text_width(label, label_scale);
    let lx = x + (w.saturating_sub(lw)) / 2;
    let ly = y + bt + label_scale.min(4);
    fb.draw_text(
        lx,
        ly,
        label,
        p.label_color.0,
        p.label_color.1,
        p.label_color.2,
        label_scale,
    );

    let divider_y = ly + Framebuffer::text_height(label_scale) + 4;
    fb.fill_rect(
        x + bt,
        divider_y,
        w.saturating_sub(bt * 2),
        bt,
        p.border.0,
        p.border.1,
        p.border.2,
    );

    Rect {
        x,
        y: divider_y,
        w,
        h: (y + h).saturating_sub(divider_y),
    }
}

/// Nilai besar di tengah kotak berbingkai (dengan auto-fit skala teks).
pub(super) fn draw_big_value(
    fb: &mut Framebuffer,
    rect: Rect,
    value: &str,
    p: Palette,
    max_scale: u32,
) {
    let Rect { x, y, w, h } = rect;
    let bt = p.border_thickness;
    fill_panel(fb, rect, p);

    let vs = fit_text_scale(
        value,
        w.saturating_sub(bt * 2 + 4),
        max_scale.min(h.saturating_sub(bt * 2) / Framebuffer::text_height(1)),
    );
    let vw = Framebuffer::text_width(value, vs);
    let vx = x + (w.saturating_sub(vw)) / 2;
    let vy = y + (h.saturating_sub(Framebuffer::text_height(vs))) / 2;
    fb.draw_text(
        vx,
        vy,
        value,
        p.value_color.0,
        p.value_color.1,
        p.value_color.2,
        vs,
    );
}

/// Sel metrik berbingkai: label kecil di tengah atas, nilai besar di tengah
/// (auto-fit), dan unit opsional di bawah nilai (mis. "T/S", "KB/S").
pub(super) fn draw_metric_cell(
    fb: &mut Framebuffer,
    rect: Rect,
    label: &str,
    value: &str,
    unit: Option<&str>,
    p: Palette,
    value_scale: u32,
    detail_scale: u32,
) {
    let Rect { x, y, w, h } = rect;
    let bt = p.border_thickness;
    fill_panel(fb, rect, p);

    let line_height = Framebuffer::text_height(1);
    let inner_h = h.saturating_sub(bt * 2);
    let inner_w = w.saturating_sub(bt * 2 + 4);
    let lines = if unit.is_some() { 3 } else { 2 };
    let gap = detail_scale.min(2);
    let content_h = inner_h.saturating_sub(gap * (lines - 1) + 2);
    if content_h < lines * line_height {
        let text = format!("{label} {value} {}", unit.unwrap_or(""));
        let scale = fit_text_scale(text.trim_end(), inner_w, inner_h / line_height);
        if inner_h >= line_height {
            fb.draw_text_clipped(
                (x + bt) as i64,
                y + (h - Framebuffer::text_height(scale)) / 2,
                text.trim_end(),
                p.value_color.0,
                p.value_color.1,
                p.value_color.2,
                scale,
                x + bt,
                x + w - bt,
            );
        }
        return;
    }
    let caption_cap = (content_h / (lines * line_height)).min(detail_scale);
    let lab_scale = fit_text_scale(label, inner_w, caption_cap);
    let unit_scale = unit.map(|u| fit_text_scale(u, inner_w, caption_cap));
    let label_h = Framebuffer::text_height(lab_scale);
    let unit_h = unit_scale.map_or(0, Framebuffer::text_height);
    let val_scale = fit_text_scale(
        value,
        inner_w,
        value_scale.min((content_h - label_h - unit_h) / line_height),
    );
    let value_h = Framebuffer::text_height(val_scale);
    let mut line = |text: &str, scale: u32, line_y: u32, color: (u8, u8, u8)| {
        let left = x + w.saturating_sub(Framebuffer::text_width(text, scale)) / 2;
        fb.draw_text_clipped(
            left as i64,
            line_y,
            text,
            color.0,
            color.1,
            color.2,
            scale,
            x + bt,
            x + w - bt,
        );
    };
    let top = y + bt + 1;
    line(label, lab_scale, top, p.label_color);
    let value_y = top + label_h + gap + content_h.saturating_sub(label_h + value_h + unit_h) / 2;
    line(value, val_scale, value_y, p.value_color);
    if let (Some(u), Some(scale)) = (unit, unit_scale) {
        line(u, scale, value_y + value_h + gap, p.label_color);
    }
}

/// Format kecepatan jaringan: di bawah 1024 → KB/S bulat, di atasnya
/// MB/S dengan satu desimal.
fn fmt_net_speed(kb_s: f64) -> (String, &'static str) {
    if kb_s >= 1024.0 {
        (format!("{:.1}", kb_s / 1024.0), "MB/S")
    } else {
        (format!("{:.0}", kb_s), "KB/S")
    }
}

/// Panel metrik (CPU / GPU / MEM): label + nilai besar (mis. persentase) +
/// dua sel kecil di bawahnya (mis. TEMP | WATT).
fn draw_metric_panel(
    fb: &mut Framebuffer,
    rect: Rect,
    label: &str,
    big_value: &str,
    cells: &[(&str, String); 2],
    p: Palette,
    value_scale: u32,
    detail_scale: u32,
) {
    let Rect {
        x: ix,
        y: iy,
        w: iw,
        h: ih,
    } = draw_panel_chrome(fb, rect, label, p, detail_scale);
    let bt = p.border_thickness;

    let big_h = if ih < 60 { ih / 3 } else { ih * 45 / 100 };
    draw_big_value(
        fb,
        Rect {
            x: ix,
            y: iy,
            w: iw,
            h: big_h,
        },
        big_value,
        p,
        value_scale,
    );

    let cells_y = iy + big_h.saturating_sub(bt);
    let cells_h = (iy + ih).saturating_sub(cells_y);
    let cw = (iw + bt) / 2;
    // Skala nilai sel kecil dikurangi supaya ada jarak vertikal dari label
    // (di panel 1/3 tinggi, nilai skala penuh menempel persis di bawah label).
    let cell_value_scale = value_scale.saturating_sub(2);
    for (c, (cl, cv)) in cells.iter().enumerate() {
        let cx = ix + c as u32 * cw.saturating_sub(bt);
        let cww = if c == 1 {
            (ix + iw).saturating_sub(cx)
        } else {
            cw
        };
        draw_metric_cell(
            fb,
            Rect {
                x: cx,
                y: cells_y,
                w: cww,
                h: cells_h,
            },
            cl,
            cv,
            None,
            p,
            cell_value_scale,
            detail_scale,
        );
    }
}

/// Panel NET: dua sel selebar area dalam (DL | UP), masing-masing dengan unit.
fn draw_net_panel(
    fb: &mut Framebuffer,
    rect: Rect,
    title: &str,
    dl: (&str, String, &'static str),
    up: (&str, String, &'static str),
    p: Palette,
    value_scale: u32,
    detail_scale: u32,
) {
    let Rect {
        x: ix,
        y: iy,
        w: iw,
        h: ih,
    } = draw_panel_chrome(fb, rect, title, p, detail_scale);
    let bt = p.border_thickness;
    let cw = (iw + bt) / 2;
    // Nilai DL/UP sedikit lebih kecil dari nilai besar panel lain — tetap
    // terbaca, tapi tidak menempel ke labelnya.
    let cell_value_scale = value_scale.saturating_sub(1);
    draw_metric_cell(
        fb,
        Rect {
            x: ix,
            y: iy,
            w: cw,
            h: ih,
        },
        dl.0,
        &dl.1,
        Some(dl.2),
        p,
        cell_value_scale,
        detail_scale,
    );
    let up_x = ix + cw.saturating_sub(bt);
    let up_w = (ix + iw).saturating_sub(up_x);
    draw_metric_cell(
        fb,
        Rect {
            x: up_x,
            y: iy,
            w: up_w,
            h: ih,
        },
        up.0,
        &up.1,
        Some(up.2),
        p,
        cell_value_scale,
        detail_scale,
    );
}

/// Panel AI INFERENCE: baris model+status (marquee) di atas, lalu sel-sel
/// metrik inferensi. Layout sel: 4 sebaris kalau lebar, 2x2 kalau sempit.
fn draw_ai_panel(
    fb: &mut Framebuffer,
    rect: Rect,
    ai: &inferrig::InferrigState,
    marquee: &mut Marquee,
    p: Palette,
    value_scale: u32,
    detail_scale: u32,
) {
    let Rect {
        x: ix,
        y: iy,
        w: iw,
        h: ih,
    } = draw_panel_chrome(fb, rect, "AI INFERENCE", p, detail_scale);
    let bt = p.border_thickness;
    let song_scale = detail_scale;

    // ── Baris model + state ──
    let model_area_h = Framebuffer::text_height(song_scale) + bt * 2 + 8 * detail_scale;
    let cells_area_h = ih.saturating_sub(model_area_h).saturating_sub(bt);

    fb.fill_rect(ix, iy, iw, model_area_h, p.border.0, p.border.1, p.border.2);
    fb.fill_rect(
        ix + bt,
        iy + bt,
        iw.saturating_sub(bt * 2),
        model_area_h.saturating_sub(bt * 2),
        p.panel_bg.0,
        p.panel_bg.1,
        p.panel_bg.2,
    );

    let (model_text, m_color) = if let Some(state) = &ai.lifecycle {
        (
            format!(
                "{} ({})",
                state.active_model.as_deref().unwrap_or("-"),
                state.state
            ),
            p.value_color,
        )
    } else {
        ("INFERRIG OFFLINE".to_string(), p.label_color)
    };
    let clip_x0 = ix + bt + 4;
    let clip_x1 = (ix + iw).saturating_sub(bt + 4);
    let available_width = clip_x1.saturating_sub(clip_x0);
    let text_h = Framebuffer::text_height(song_scale);
    let text_y = iy + bt + (model_area_h.saturating_sub(bt + text_h)) / 2;
    if available_width > 0 && marquee.tick(&model_text, available_width, song_scale) {
        let loop_text = format!("{}{}", model_text, MARQUEE_GAP);
        let loop_width = Framebuffer::text_width(&loop_text, song_scale) as i64;
        let base_x = clip_x0 as i64 - marquee.offset_px as i64;
        fb.draw_text_clipped(
            base_x, text_y, &loop_text, m_color.0, m_color.1, m_color.2, song_scale, clip_x0,
            clip_x1,
        );
        fb.draw_text_clipped(
            base_x + loop_width,
            text_y,
            &loop_text,
            m_color.0,
            m_color.1,
            m_color.2,
            song_scale,
            clip_x0,
            clip_x1,
        );
    } else {
        let tw = Framebuffer::text_width(&model_text, song_scale);
        let tx = ix + (iw.saturating_sub(tw)) / 2;
        fb.draw_text(
            tx,
            text_y,
            &model_text,
            m_color.0,
            m_color.1,
            m_color.2,
            song_scale,
        );
    }

    // ── Sel metrik ──
    let telemetry = &ai.telemetry;
    let available = telemetry.error.is_empty();
    let rate = |v: Option<f64>| {
        v.filter(|_| available)
            .map_or_else(|| "N/A".into(), |v| format!("{v:.1}"))
    };
    let requests = |v: Option<u64>| {
        v.filter(|_| available)
            .map_or_else(|| "N/A".into(), |v| v.to_string())
    };
    let gpu = telemetry
        .host
        .as_ref()
        .filter(|h| h.error.is_empty())
        .and_then(|h| h.gpu_memory.as_ref());
    let memory_label = match gpu.and_then(|g| g.kind.as_deref()) {
        Some("gtt") => "GTT",
        Some("vram") => "VRAM",
        _ => "GPU MEM",
    };
    let cells = [
        ("GEN", rate(telemetry.generation_tokens_per_second), "T/S"),
        ("PROMPT", rate(telemetry.prompt_tokens_per_second), "T/S"),
        (
            "REQ",
            format!(
                "{}+{}",
                requests(telemetry.requests_active),
                requests(telemetry.requests_queued)
            ),
            "",
        ),
        (
            memory_label,
            gpu.and_then(|g| g.used_bytes).map_or_else(
                || "N/A".into(),
                |v| format!("{:.1}", v as f64 / 1073741824.0),
            ),
            "GiB",
        ),
    ];

    let cells_y = iy + model_area_h;
    let wide = iw >= 900 || ih < 160;
    let (cols, rows): (u32, u32) = if wide { (4, 1) } else { (2, 2) };
    let cw = (iw + bt * (cols - 1)) / cols;
    let ch = (cells_area_h + bt * (rows - 1)) / rows;
    let cell_value_scale = value_scale.saturating_sub(1);
    for (i, (label, value, unit)) in cells.iter().enumerate() {
        let col = (i as u32) % cols;
        let row = (i as u32) / cols;
        let cx = ix + col * cw.saturating_sub(bt);
        let cy = cells_y + row * ch.saturating_sub(bt);
        let cww = if col + 1 == cols {
            (ix + iw).saturating_sub(cx)
        } else {
            cw
        };
        let chh = if row + 1 == rows {
            (iy + ih).saturating_sub(cy)
        } else {
            ch
        };
        let unit = if unit.is_empty() { None } else { Some(*unit) };
        draw_metric_cell(
            fb,
            Rect {
                x: cx,
                y: cy,
                w: cww,
                h: chh,
            },
            label,
            value,
            unit,
            p,
            cell_value_scale,
            detail_scale,
        );
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Rect {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) w: u32,
    pub(super) h: u32,
}

impl Rect {
    pub(super) fn col(self, index: u32, count: u32) -> Self {
        let start = self.w * index / count;
        Self {
            x: self.x + start,
            w: self.w * (index + 1) / count - start,
            ..self
        }
    }

    pub(super) fn row(self, index: u32, count: u32) -> Self {
        let start = self.h * index / count;
        Self {
            y: self.y + start,
            h: self.h * (index + 1) / count - start,
            ..self
        }
    }

    pub(super) fn inset(self, dx: u32, dy: u32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            w: self.w.saturating_sub(2 * dx),
            h: self.h.saturating_sub(2 * dy),
        }
    }
}

/// Four system panels share the available space with an optional inference panel.
pub(super) fn inference_layout(
    width: u32,
    height: u32,
    portrait: bool,
    ai: bool,
) -> ([Rect; 4], Option<Rect>) {
    let gap = if portrait { 8 } else { 10 };
    let margin = if portrait { 8 } else { 16 };
    let w = width - 2 * margin;
    let usable_h = height - 20;
    let system_h = if !ai {
        usable_h
    } else if portrait {
        (usable_h - 4 * gap) * 70 / 100 + 3 * gap
    } else {
        (usable_h - gap) / 3
    };
    let panels = std::array::from_fn(|i| {
        let i = i as u32;
        if portrait {
            let space = system_h - 3 * gap;
            Rect {
                x: margin,
                y: 10 + i * gap + space * i / 4,
                w,
                h: space * (i + 1) / 4 - space * i / 4,
            }
        } else {
            let space = w - 3 * gap;
            Rect {
                x: margin + i * gap + space * i / 4,
                y: 10,
                w: space * (i + 1) / 4 - space * i / 4,
                h: system_h,
            }
        }
    });
    let ai_rect = ai.then(|| Rect {
        x: margin,
        y: 10 + system_h + gap,
        w,
        h: usable_h - system_h - gap,
    });
    (panels, ai_rect)
}

pub(super) fn draw_inference(
    fb: &mut Framebuffer,
    data: &Readings<'_>,
    peak: bool,
    ai: Option<&inferrig::InferrigState>,
    marquee: &mut Marquee,
    mut p: Palette,
    rotation: Rotation,
) {
    let Readings {
        sys,
        gpu_percent,
        gpu: gpu_data,
        net: net_kb,
        cpu_temp,
        cpu_power,
        ..
    } = *data;
    let (panels, ai_rect) =
        inference_layout(fb.width(), fb.height(), is_portrait(rotation), ai.is_some());
    let compact = panels[0].h < 130 || panels[0].w < 145;
    let (value_scale, detail_scale) = if compact { (2, 1) } else { (5, 3) };
    if compact {
        p.border_thickness = 1;
    }
    let cpu = sys.global_cpu_info().cpu_usage();
    // Memori dalam GiB utuh, dibulatkan ke ATAS (34.1G dan 34.7G -> 35G).
    let used_gb = (sys.used_memory() as f64 / 1073741824.0).ceil() as u64;
    let total_gb = (sys.total_memory() as f64 / 1073741824.0).ceil() as u64;
    let mem_pct = if sys.total_memory() > 0 {
        (sys.used_memory() as f32 / sys.total_memory() as f32) * 100.0
    } else {
        0.0
    };

    let gpu_str = match gpu_percent {
        Some(v) => format!("{v:.0}%"),
        None => "N/A".to_string(),
    };
    let cpu_temp_str = cpu_temp.map_or_else(|| "N/A".to_string(), |t| format!("{t:.0}C"));
    let cpu_watt_str = cpu_power.map_or_else(|| "N/A".to_string(), |w| format!("{w:.0}W"));
    let gpu_temp_str = gpu_data
        .temp_edge_c
        .map_or_else(|| "N/A".to_string(), |t| format!("{t:.0}C"));
    let gpu_watt_str = gpu_data
        .power_w
        .map_or_else(|| "N/A".to_string(), |w| format!("{w:.0}W"));

    let (net_down, net_up) = net_kb;
    let (dl_val, dl_unit) = fmt_net_speed(net_down);
    let (up_val, up_unit) = fmt_net_speed(net_up);

    let metric_panels: [(&str, String, [(&str, String); 2]); 3] = [
        (
            "CPU",
            format!("{cpu:.0}%"),
            [("TEMP", cpu_temp_str), ("WATT", cpu_watt_str)],
        ),
        (
            "GPU",
            gpu_str,
            [("TEMP", gpu_temp_str), ("WATT", gpu_watt_str)],
        ),
        (
            "MEM",
            format!("{mem_pct:.0}%"),
            [
                ("USED", format!("{used_gb}G")),
                ("TOTAL", format!("{total_gb}G")),
            ],
        ),
    ];

    for (rect, (label, big, cells)) in panels.iter().zip(&metric_panels) {
        draw_metric_panel(fb, *rect, label, big, cells, p, value_scale, detail_scale);
    }
    let net = panels[3];
    draw_net_panel(
        fb,
        net,
        if peak { "NET PEAK" } else { "NET" },
        ("DL", dl_val, dl_unit),
        ("UP", up_val, up_unit),
        p,
        value_scale,
        detail_scale,
    );
    if let (Some(rect), Some(snapshot)) = (ai_rect, ai) {
        draw_ai_panel(fb, rect, snapshot, marquee, p, value_scale, detail_scale);
    }
}
