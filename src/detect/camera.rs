//! Video4Linux camera enumeration and IR detection.

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Camera {
    /// e.g. "/dev/video2"
    pub node: String,
    pub name: String,
    pub is_ir: bool,
    pub usb_id: Option<String>,
}

/// Heuristic: UVC IR cameras almost always say "IR" or "Infrared" in their name.
pub fn looks_ir(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("infrared")
        || lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w == "ir")
}

fn node_number(node: &str) -> u32 {
    node.trim_start_matches("/dev/video")
        .parse()
        .unwrap_or(u32::MAX)
}

pub fn scan() -> Vec<Camera> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") else {
        return out;
    };
    for entry in entries.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        if !fname.starts_with("video") {
            continue;
        }
        let dir = entry.path();
        // UVC exposes a capture node (index 0) plus metadata nodes; keep capture only.
        let index = std::fs::read_to_string(dir.join("index")).unwrap_or_default();
        if index.trim() != "0" && !index.trim().is_empty() {
            continue;
        }
        let name = std::fs::read_to_string(dir.join("name"))
            .unwrap_or_default()
            .trim()
            .to_string();
        // "HP HD Camera: HP IR Camera" -> the part after the colon is the function name.
        let display = name
            .split_once(": ")
            .map(|(_, b)| b.to_string())
            .unwrap_or(name.clone());
        let usb_id = usb_parent_id(&dir.join("device"));
        out.push(Camera {
            node: format!("/dev/{fname}"),
            is_ir: looks_ir(&name),
            name: display,
            usb_id,
        });
    }
    out.sort_by_key(|c| node_number(&c.node));
    out
}

fn usb_parent_id(device_link: &Path) -> Option<String> {
    let real = std::fs::canonicalize(device_link).ok()?;
    real.ancestors()
        .find_map(|dir| crate::detect::usb::read_usb_device(dir).map(|d| d.id_string()))
}

#[cfg(test)]
mod tests {
    use super::looks_ir;

    #[test]
    fn ir_heuristic() {
        assert!(looks_ir("HP HD Camera: HP IR Camera"));
        assert!(looks_ir("Integrated Infrared Camera"));
        assert!(looks_ir("IR Camera"));
        assert!(!looks_ir("HP HD Camera: HP HD Camera"));
        assert!(!looks_ir("Integrated Camera (Mirror)"));
    }
}
