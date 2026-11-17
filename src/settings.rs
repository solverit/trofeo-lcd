//! Dashboard preferences. CLI values override the file, then the selected profile supplies defaults.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    #[default]
    Grid,
    Inference,
    Classic,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Standard,
    Terminal,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub profile: Option<Profile>,
    pub theme: Option<Theme>,
    pub idle_fps: Option<f32>,
    pub active_fps: Option<f32>,
    pub audio: Option<bool>,
    pub inferrig_url: Option<String>,
    /// Zero displays the current transfer rate. Positive values select peak display.
    pub net_peak_half_life_secs: Option<f64>,
}

fn default_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.map(|base| base.join("trofeo-lcd/config.json"))
}

pub fn load(explicit: Option<&Path>) -> Result<Settings> {
    let default = default_path();
    let Some(path) = explicit.or(default.as_deref()) else {
        return Ok(Settings::default());
    };
    match std::fs::read(path) {
        Ok(data) => serde_json::from_slice(&data)
            .with_context(|| format!("Invalid configuration in {}", path.display())),
        Err(e) if explicit.is_none() && e.kind() == std::io::ErrorKind::NotFound => {
            Ok(Settings::default())
        }
        Err(e) => Err(e).with_context(|| format!("Cannot read configuration {}", path.display())),
    }
}
