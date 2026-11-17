//! Driver Rust untuk perangkat LCD HID "Type 2" Thermalright — Winbond
//! USBDISPLAY, **VID:PID `0416:5302`**.
//!
//! Revisi firmware baru dari beberapa panel Thermalright (termasuk unit
//! Trofeo Vision) tidak lagi memakai protokol bulk "LY" (`0416:5408`), tapi
//! varian HID: transfer dilakukan sebagai **HID output report** 512-byte
//! lewat `/dev/hidraw*` (crate `hidapi`), BUKAN bulk transfer libusb —
//! firmware 4.07 terbukti mengabaikan chunk yang sama persis kalau dikirim
//! sebagai bulk (lihat issue #228 thermalright-trcc-linux).
//!
//! Port dari `thermalright-trcc-linux`:
//! - `src/trcc/adapters/device/hid_lcd.py` (kelas `HidLcd`, device_type=2)
//! - tabel PM/FBL/rotasi dari `src/trcc/core/protocol.py`
//!
//! # Protokol (Type 2)
//! - **Handshake**: kirim packet 512-byte `DA DB DC DD` + 8 nol + `01 00 00
//!   00` + 4 nol (zero-padded), lalu baca respons. Valid: `resp[0..4]` ==
//!   magic. Firmware biasa menjawab ≥20 byte dengan `resp[12] == 0x01`;
//!   firmware "streaming" menjawab pendek 8 byte. **PM = resp[5], SUB =
//!   resp[4]** — dari situ resolusi & rotasi wire dilihat lewat tabel.
//! - **Frame**: header 20-byte (`DA DB DC DD 02 00` + mode `00 00` untuk
//!   JPEG + `w,h` u16 LE (resolusi device) + `02 00 00 00` + panjang data
//!   u32 LE) + bytes JPEG, zero-padded ke kelipatan 512, dikirim sebagai
//!   rangkaian HID report 512-byte (report-ID `0x00` di depan tiap write),
//!   lalu delay 1 ms. Panel ini butuh stream frame kontinu — kalau tidak
//!   dikirimi frame lagi, panel kembali ke boot logo.
//! - Batas JPEG firmware sama dengan LY: 450.000 byte.

use std::time::Duration;

use crate::{Framebuffer, LcdError, Resolution, Result, Rotation, VENDOR_ID, MAX_FRAME_BYTES};

use hidapi::{HidApi, HidDevice};

/// PID varian HID (Winbond USBDISPLAY).
pub const HID_PID: u16 = 0x5302;

const MAGIC: [u8; 4] = [0xDA, 0xDB, 0xDC, 0xDD];
/// Ukuran satu HID report (dan alignment frame).
const REPORT_SIZE: usize = 512;
/// Delay setelah satu frame utuh terkirim (konstanta C# `ThreadSendDeviceData2`).
const FRAME_DELAY: Duration = Duration::from_millis(1);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(3000);
/// Retry tulis chunk kalau firmware sementara backpressure.
const WRITE_RETRIES: usize = 5;

/// Hasil handshake + info device yang sudah di-resolve.
#[derive(Debug, Clone)]
pub struct HidHandshake {
    pub pm: u8,
    pub sub: u8,
    pub width: u32,
    pub height: u32,
    /// Rotasi wire (derajat, searah jarum jam) dari tabel C# ENCODE_ROTATIONS
    /// untuk resolusi+SUB ini — dipakai sebagai default `--hid-rotate`.
    pub wire_rotation: u32,
}

/// Perangkat LCD HID Type 2 yang sudah terbuka + handshake sukses.
pub struct HidLcd {
    device: HidDevice,
    handshake: HidHandshake,
    /// Override manual rotasi wire (`--hid-rotate`), kalau diberikan.
    rotation_override: Option<u32>,
}

impl HidLcd {
    /// Buka perangkat HID 0416:5302 pertama yang ditemukan, jalankan
    /// handshake, dan resolve resolusi dari PM/SUB.
    ///
    /// `rotation_override` (derajat 0/90/180/270) menggantikan rotasi wire
    /// hasil tabel — dipakai kalau ternyata orientasi di glass Anda berbeda.
    pub fn open(rotation_override: Option<u32>) -> Result<Self> {
        let api = HidApi::new().map_err(|e| {
            eprintln!("hidapi: gagal init ({e})");
            LcdError::NotFound
        })?;
        let info = api
            .device_list()
            .find(|d| d.vendor_id() == VENDOR_ID && d.product_id() == HID_PID)
            .ok_or(LcdError::NotFound)?;
        let device = info
            .open_device(&api)
            .map_err(|e| LcdError::Hid(format!("gagal buka 0416:5302: {e}")))?;

        let mut lcd = Self {
            device,
            handshake: HidHandshake {
                pm: 0,
                sub: 0,
                width: 0,
                height: 0,
                wire_rotation: 0,
            },
            rotation_override,
        };
        lcd.handshake()?;
        Ok(lcd)
    }

    pub fn handshake_info(&self) -> &HidHandshake {
        &self.handshake
    }

    pub fn resolution(&self) -> Resolution {
        Resolution::new(self.handshake.width, self.handshake.height)
    }

    /// Rotasi wire yang benar-benar dipakai: override manual atau hasil tabel.
    fn wire_rotation(&self) -> u32 {
        self.rotation_override
            .unwrap_or(self.handshake.wire_rotation)
    }

    /// Handshake Type 2: kirim init packet 512-byte, baca respons, parse
    /// PM/SUB, lalu resolve resolusi + rotasi wire dari tabel PM/FBL.
    fn handshake(&mut self) -> Result<()> {
        let mut init = [0u8; REPORT_SIZE];
        init[0..4].copy_from_slice(&MAGIC);
        init[12] = 0x01;

        self.hid_write(&init)?;
        let resp = self.hid_read(HANDSHAKE_TIMEOUT)?;

        if resp.len() < 6 || resp[0..4] != MAGIC {
            return Err(LcdError::BadHandshake(resp));
        }
        // Firmware biasa: resp[12] == 0x01 dan len >= 20. Firmware streaming
        // menjawab 8 byte — magic saja sudah cukup identifikasi (port perilaku
        // `_validate_response_type2` dengan quirk short_handshake).
        let sub = resp[4];
        let pm = resp[5];
        if pm == 0 {
            return Err(LcdError::BadHandshake(resp));
        }

        let (width, height) = resolve_resolution(pm, sub);
        let wire_rotation = resolve_wire_rotation(width, height, sub);
        self.handshake = HidHandshake {
            pm,
            sub,
            width,
            height,
            wire_rotation,
        };
        Ok(())
    }

    /// Tulis satu HID report (512-byte data). Report-ID 0x00 ditambahkan
    /// otomatis di depan (perangkat ini tidak pakai numbered reports).
    fn hid_write(&self, data: &[u8]) -> Result<()> {
        debug_assert_eq!(data.len() % REPORT_SIZE, 0);
        let mut buf = Vec::with_capacity(data.len() + 1);
        buf.push(0x00);
        buf.extend_from_slice(data);

        let mut last_err = None;
        for _ in 0..WRITE_RETRIES {
            match self.device.write(&buf) {
                Ok(n) if n >= buf.len() => return Ok(()),
                Ok(n) => last_err = Some(format!("short write ({n}/{} byte)", buf.len())),
                Err(e) => last_err = Some(format!("{e}")),
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Err(LcdError::Hid(format!(
            "write gagal setelah {WRITE_RETRIES} percobaan: {}",
            last_err.unwrap_or_default()
        )))
    }

    /// Baca respons dari interrupt IN (blocking sampai timeout).
    fn hid_read(&self, timeout: Duration) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; REPORT_SIZE];
        let n = self
            .device
            .read_timeout(&mut buf, (timeout.as_millis() as u64).min(i32::MAX as u64) as i32)
            .map_err(|e| LcdError::Hid(format!("HID read gagal: {e}")))?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Kirim satu payload JPEG (sudah di-encode). Menyusun header Type 2,
    /// padding ke kelipatan 512, lalu menulis per-report 512-byte.
    pub fn send_jpeg(&self, jpeg: &[u8]) -> Result<()> {
        if jpeg.is_empty() {
            return Err(LcdError::EmptyFrame);
        }
        if jpeg.len() > MAX_FRAME_BYTES {
            return Err(LcdError::FrameTooLarge(jpeg.len()));
        }

        let hs = &self.handshake;
        let mut frame = Vec::with_capacity(20 + jpeg.len() + REPORT_SIZE);
        frame.extend_from_slice(&MAGIC);
        frame.extend_from_slice(&[0x02, 0x00]); // cmd_type = PICTURE
        frame.extend_from_slice(&[0x00, 0x00]); // mode = JPEG
        frame.extend_from_slice(&(hs.width as u16).to_le_bytes());
        frame.extend_from_slice(&(hs.height as u16).to_le_bytes());
        frame.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]);
        frame.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
        frame.extend_from_slice(jpeg);
        while frame.len() % REPORT_SIZE != 0 {
            frame.push(0);
        }

        for chunk in frame.chunks_exact(REPORT_SIZE) {
            self.hid_write(chunk)?;
        }
        std::thread::sleep(FRAME_DELAY);
        Ok(())
    }

    /// Rangkaian: terapkan rotasi user (`--rotate`, termasuk 180°) lalu
    /// rotasi wire panel (default dari tabel PM/SUB, atau `--hid-rotate`),
    /// encode JPEG, kirim.
    pub fn send_framebuffer(&self, fb: &Framebuffer, quality: u8, rotation: Rotation) -> Result<()> {
        let owned180 = if rotation == Rotation::Deg180 {
            Some(fb.rotated_180())
        } else {
            None
        };
        let stage1 = owned180.as_ref().unwrap_or(fb);

        let jpeg = match self.wire_rotation() % 360 {
            90 => stage1.rotated_90().to_jpeg(quality)?,
            180 => stage1.rotated_180().to_jpeg(quality)?,
            270 => stage1.rotated_270().to_jpeg(quality)?,
            _ => stage1.to_jpeg(quality)?,
        };
        self.send_jpeg(&jpeg)
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Tabel PM/FBL — port dari `core/protocol.py` thermalright-trcc-linux
// (FBL_PROFILES + _PM_TO_FBL_OVERRIDES + _FBL_192/224_BY_PM). Cukup baris
// yang relevan untuk panel JPEG; PM lain (RGB565 kecil) tidak mungkin
// muncul di 0416:5302 versi firmware ini, tapi tetap dimapping ke ukuran
// yang benar supaya frame tidak kacau.
// ─────────────────────────────────────────────────────────────────────────

/// PM byte → FBL byte (konvensi SCSI: PM == FBL, kecuali override ini).
fn pm_to_fbl(pm: u8, sub: u8) -> u8 {
    match (pm, sub) {
        (1, 49) => 192,
        (1, 48) => 114,
        _ => match pm {
            5 => 50,
            7 => 64,
            9 | 11 | 20 => 224,
            10 => 224,
            12 => 224,
            13 | 17 | 18 => 224,
            14 => 64,
            15 => 224,
            16 => 224,
            32 => 100,
            50 => 50,
            63 => 114,
            64 => 114,
            65 => 192,
            66 => 192,
            68 => 192,
            69 => 192,
            other => other,
        },
    }
}

/// FBL (+ PM untuk FBL 192/224 yang dipakai beberapa resolusi) → (w, h).
/// Semua panel yang bisa muncul di jalur ini JPEG; kalau FBL tidak dikenal,
/// fallback 1280x480 (panel paling umum untuk 0416:5302 Trofeo Vision).
fn resolve_resolution(pm: u8, sub: u8) -> (u32, u32) {
    let fbl = pm_to_fbl(pm, sub);
    match fbl {
        36 | 37 => (240, 240),
        49 | 50 | 51 | 52 | 53 | 58 => (320, 240),
        54 => (360, 360),
        60 => (320, 176),
        59 => (640, 172),
        64 => (640, 480),
        72 | 129 => (480, 480),
        100 | 101 | 102 => (320, 320),
        114 => (1600, 720),
        128 => (1280, 480),
        192 => match pm {
            68 => (1280, 480),
            69 => (1920, 440),
            _ => (1920, 462),
        },
        224 => match pm {
            9 | 11 | 20 => (854, 480),
            10 | 16 => (960, 540),
            12 => (800, 480),
            13 | 17 | 18 => (960, 320),
            15 => (640, 172),
            _ => (854, 480),
        },
        _ => (1280, 480),
    }
}

/// Rotasi wire frame (derajat, searah jarum jam) sebelum encode JPEG — port
/// tabel `ENCODE_ROTATIONS` (baris ImageToJpg) pada orientation user = 0.
fn resolve_wire_rotation(w: u32, h: u32, sub: u8) -> u32 {
    match (w, h) {
        (1600, 720) => {
            if sub == 3 {
                0
            } else {
                180
            }
        }
        (1280, 480) => {
            if sub == 2 {
                90
            } else {
                0
            }
        }
        (1920, 462) | (1920, 440) => {
            if (2..=4).contains(&sub) {
                0
            } else {
                180
            }
        }
        (854, 480) | (800, 480) => 0,
        (960, 540) => {
            if sub == 5 || sub == 7 {
                180
            } else {
                0
            }
        }
        (960, 320) => {
            if sub < 5 {
                0
            } else {
                180
            }
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_trofeo_vision_hid_pm128_sub2() {
        // Unit nyata di mesin ini: PM=128 SUB=2 → 1280x480, wire 90°.
        assert_eq!(resolve_resolution(128, 2), (1280, 480));
        assert_eq!(resolve_wire_rotation(1280, 480, 2), 90);
        assert_eq!(resolve_wire_rotation(1280, 480, 1), 0);
    }

    #[test]
    fn pm65_maps_to_1920x462() {
        assert_eq!(resolve_resolution(65, 0), (1920, 462));
        assert_eq!(resolve_wire_rotation(1920, 462, 0), 180);
        assert_eq!(resolve_wire_rotation(1920, 462, 3), 0);
    }

    #[test]
    fn header_layout_matches_csharp() {
        // 20-byte header: magic + 02 00 + 00 00 + w,h LE + 02 00 00 00 + len LE.
        let hs = HidHandshake {
            pm: 128,
            sub: 2,
            width: 1280,
            height: 480,
            wire_rotation: 90,
        };
        let mut frame = Vec::new();
        frame.extend_from_slice(&MAGIC);
        frame.extend_from_slice(&[0x02, 0x00]);
        frame.extend_from_slice(&[0x00, 0x00]);
        frame.extend_from_slice(&(hs.width as u16).to_le_bytes());
        frame.extend_from_slice(&(hs.height as u16).to_le_bytes());
        frame.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]);
        frame.extend_from_slice(&1234u32.to_le_bytes());
        assert_eq!(frame.len(), 20);
        assert_eq!(&frame[0..4], &[0xDA, 0xDB, 0xDC, 0xDD]);
        assert_eq!(frame[4], 0x02);
        assert_eq!(frame[6], 0x00);
        assert_eq!(frame[8..10], 1280u16.to_le_bytes());
        assert_eq!(frame[10..12], 480u16.to_le_bytes());
        assert_eq!(frame[12], 0x02);
        assert_eq!(frame[16..20], 1234u32.to_le_bytes());
    }
}
