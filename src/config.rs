//! Per-user settings and the fingerprint device knowledge base.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const BUILTIN_DEVICES: &str = include_str!("../data/devices.toml");

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// A python-validity source checkout used for sensors that need the
    /// "local" install path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validity_checkout: Option<PathBuf>,
}

fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("simple-auth-tool"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/simple-auth-tool"))
}

impl Settings {
    pub fn load() -> Self {
        config_dir()
            .and_then(|d| std::fs::read_to_string(d.join("config.toml")).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let dir = config_dir().ok_or_else(|| anyhow::anyhow!("no HOME directory"))?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("config.toml"), toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KnownBackend {
    Libfprint,
    PythonValidity,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValiditySource {
    Ppa,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownDevice {
    pub usb: String,
    pub name: String,
    pub backend: KnownBackend,
    #[serde(default)]
    pub validity: Option<ValiditySource>,
    #[serde(default)]
    pub notes: Option<String>,
}

impl KnownDevice {
    pub fn usb_id(&self) -> Option<(u16, u16)> {
        let (v, p) = self.usb.split_once(':')?;
        Some((
            u16::from_str_radix(v, 16).ok()?,
            u16::from_str_radix(p, 16).ok()?,
        ))
    }
}

#[derive(Debug, Default, Deserialize)]
struct DeviceFile {
    #[serde(default)]
    device: Vec<KnownDevice>,
}

pub fn parse_devices(text: &str) -> anyhow::Result<Vec<KnownDevice>> {
    Ok(toml::from_str::<DeviceFile>(text)?.device)
}

/// User entries first so they take precedence, then the built-in ones.
pub fn known_devices() -> Vec<KnownDevice> {
    let mut all = Vec::new();
    if let Some(text) =
        config_dir().and_then(|d| std::fs::read_to_string(d.join("devices.toml")).ok())
    {
        match parse_devices(&text) {
            Ok(mut d) => all.append(&mut d),
            Err(e) => log::warn!("ignoring user devices.toml: {e}"),
        }
    }
    all.extend(parse_devices(BUILTIN_DEVICES).expect("built-in devices.toml is valid"));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_database_parses() {
        let devs = parse_devices(BUILTIN_DEVICES).unwrap();
        assert!(devs.iter().all(|d| d.usb_id().is_some()));
        let b7 = devs.iter().find(|d| d.usb == "06cb:00b7").unwrap();
        assert_eq!(b7.backend, KnownBackend::PythonValidity);
        assert_eq!(b7.validity, Some(ValiditySource::Local));
    }
}
