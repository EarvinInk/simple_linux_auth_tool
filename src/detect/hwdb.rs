//! libfprint's list of supported (and known unsupported) sensors, as shipped
//! by systemd in 60-autosuspend-fingerprint-reader.hwdb.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HwdbEntry {
    Supported { driver: String },
    Unsupported,
}

const PATHS: &[&str] = &[
    "/usr/lib/udev/hwdb.d/60-autosuspend-fingerprint-reader.hwdb",
    "/lib/udev/hwdb.d/60-autosuspend-fingerprint-reader.hwdb",
    "/etc/udev/hwdb.d/60-autosuspend-fingerprint-reader.hwdb",
];

pub fn load() -> HashMap<(u16, u16), HwdbEntry> {
    PATHS
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse(&t))
        .unwrap_or_default()
}

pub fn parse(text: &str) -> HashMap<(u16, u16), HwdbEntry> {
    let mut map = HashMap::new();
    let mut section: Option<HwdbEntry> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(comment) = line.strip_prefix('#') {
            let comment = comment.trim();
            if let Some(driver) = comment.strip_prefix("Supported by libfprint driver ") {
                section = Some(HwdbEntry::Supported {
                    driver: driver.trim().to_string(),
                });
            } else if comment.starts_with("Known unsupported devices") {
                section = Some(HwdbEntry::Unsupported);
            }
            continue;
        }
        let (Some(rest), Some(entry)) = (line.strip_prefix("usb:v"), section.as_ref()) else {
            continue;
        };
        // rest looks like "06CBp00B7*"
        let Some((v, p)) = rest.split_once('p') else {
            continue;
        };
        let p = p.trim_end_matches('*');
        if let (Ok(v), Ok(p)) = (u16::from_str_radix(v, 16), u16::from_str_radix(p, 16)) {
            map.insert((v, p), entry.clone());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections() {
        let text = "# header\n\n# Supported by libfprint driver synaptics\nusb:v06CBp00BD*\nusb:v06CBp00DF*\n ID_AUTOSUSPEND=1\n\n# Known unsupported devices\nusb:v06CBp00B7*\n";
        let m = parse(text);
        assert_eq!(
            m[&(0x06cb, 0x00bd)],
            HwdbEntry::Supported {
                driver: "synaptics".into()
            }
        );
        assert_eq!(
            m[&(0x06cb, 0x00df)],
            HwdbEntry::Supported {
                driver: "synaptics".into()
            }
        );
        assert_eq!(m[&(0x06cb, 0x00b7)], HwdbEntry::Unsupported);
        assert_eq!(m.len(), 3);
    }
}
