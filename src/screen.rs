//! Shared device discovery and frame transport for both executables.
use crate::{
    hid_lcd::HidLcd, Framebuffer, LcdError, LyLcd, Resolution, Result, Rotation, TROFEO_VISION_9_16,
};

pub enum Screen {
    Ly(LyLcd),
    Hid(HidLcd),
}

impl Screen {
    pub fn open(hid_rotation: Option<u32>) -> Result<Self> {
        match LyLcd::open() {
            Ok(lcd) => {
                lcd.handshake()?;
                Ok(Self::Ly(lcd))
            }
            Err(LcdError::NotFound) => Ok(Self::Hid(HidLcd::open(hid_rotation)?)),
            Err(error) => Err(error),
        }
    }

    pub fn resolution(&self) -> Resolution {
        match self {
            Self::Ly(_) => TROFEO_VISION_9_16,
            Self::Hid(lcd) => lcd.resolution(),
        }
    }

    pub fn send_framebuffer(
        &self,
        fb: &Framebuffer,
        quality: u8,
        rotation: Rotation,
    ) -> Result<()> {
        match self {
            Self::Ly(lcd) => lcd.send_framebuffer(fb, quality, rotation),
            Self::Hid(lcd) => lcd.send_framebuffer(fb, quality, rotation),
        }
    }
}
