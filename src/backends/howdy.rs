//! Howdy (face unlock) — read-only helpers. Everything that needs root runs
//! through the privileged helper.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Config locations used by the different Howdy builds (2.x upstream, 3.x beta, distro builds).
const CONFIG_PATHS: &[&str] = &[
    "/etc/howdy/config.ini",
    "/lib/security/howdy/config.ini",
    "/usr/lib/security/howdy/config.ini",
    "/usr/local/etc/howdy/config.ini",
];

const COMPARE_PATHS: &[&str] = &[
    "/usr/lib/howdy/compare.py",
    "/lib/security/howdy/compare.py",
    "/usr/lib/security/howdy/compare.py",
    "/usr/local/lib/howdy/compare.py",
];

const PATHS_PY: &[&str] = &["/usr/lib/howdy/paths.py", "/usr/local/lib/howdy/paths.py"];

#[derive(Debug, Clone, Serialize)]
pub struct HowdyInfo {
    pub config_path: Option<String>,
    pub device_path: Option<String>,
    pub compare_py: Option<String>,
}

pub fn info() -> HowdyInfo {
    let config_path = CONFIG_PATHS
        .iter()
        .find(|p| Path::new(p).is_file())
        .map(|s| s.to_string());
    let device_path = config_path
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| ini_value(&t, "video", "device_path"));
    HowdyInfo {
        config_path,
        device_path,
        compare_py: compare_py().map(|p| p.display().to_string()),
    }
}

pub fn compare_py() -> Option<PathBuf> {
    COMPARE_PATHS
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

/// Minimal INI lookup: `[section]` then `key = value`.
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t.trim_matches(|c| c == '[' || c == ']') == section;
            continue;
        }
        if !in_section || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            if k.trim() == key {
                let v = v.trim();
                return (!v.is_empty() && v != "none").then(|| v.to_string());
            }
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FaceModel {
    pub id: u32,
    #[serde(default)]
    pub time: f64,
    #[serde(default)]
    pub label: String,
}

fn models_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // Distro builds move the models directory; read it from paths.py if present.
    for p in PATHS_PY {
        if let Ok(text) = std::fs::read_to_string(p) {
            for line in text.lines() {
                if let Some(rest) = line.trim().strip_prefix("user_models_dir") {
                    if let Some(path) = rest.split('"').nth(1) {
                        dirs.push(PathBuf::from(path));
                    }
                }
            }
        }
    }
    for d in [
        "/etc/howdy/models",
        "/lib/security/howdy/models",
        "/usr/lib/security/howdy/models",
        "/usr/share/howdy/models",
    ] {
        dirs.push(PathBuf::from(d));
    }
    dirs
}

pub enum ModelList {
    Models(Vec<FaceModel>),
    /// The model file exists but only root can read it.
    NeedsRoot,
}

/// Read a user's face models directly, if the file is readable.
pub fn list_models(user: &str) -> ModelList {
    for dir in models_dirs() {
        let file = dir.join(format!("{user}.dat"));
        match std::fs::read_to_string(&file) {
            Ok(text) => return ModelList::Models(serde_json::from_str(&text).unwrap_or_default()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return ModelList::NeedsRoot
            }
            Err(_) => continue,
        }
    }
    // Also detect an unreadable models directory.
    if models_dirs()
        .iter()
        .any(|d| d.is_dir() && std::fs::read_dir(d).is_err())
    {
        return ModelList::NeedsRoot;
    }
    ModelList::Models(Vec::new())
}

/// Parse `howdy --plain list` output: "id,YYYY-mm-dd HH:MM:SS,label".
pub fn parse_plain_list(text: &str) -> Vec<FaceModel> {
    text.lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, ',');
            let id = parts.next()?.trim().parse().ok()?;
            let _date = parts.next()?;
            Some(FaceModel {
                id,
                time: 0.0,
                label: parts.next().unwrap_or("").trim().to_string(),
            })
        })
        .collect()
}

/// Meaning of compare.py's exit codes.
pub fn compare_exit_message(code: i32) -> &'static str {
    match code {
        0 => "Face recognised",
        10 => "No face model for this user",
        11 => "Timed out: no matching face seen",
        12 => "Aborted (no user given)",
        13 => "Camera image too dark — is the IR emitter working? Check dark_threshold",
        14 => "Camera could not be opened — check device_path",
        _ => "Face test failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_lookup() {
        let t = "[core]\ndevice_path = /dev/nope\n[video]\n# device_path = /dev/video0\ndevice_path = /dev/video2\n";
        assert_eq!(
            ini_value(t, "video", "device_path").as_deref(),
            Some("/dev/video2")
        );
        assert_eq!(
            ini_value("[video]\ndevice_path = none\n", "video", "device_path"),
            None
        );
    }

    #[test]
    fn plain_list() {
        let m = parse_plain_list(
            "0,2026-01-02 10:11:12,Initial model\n1,2026-01-03 10:11:12,Glasses, dark\n\n",
        );
        assert_eq!(m.len(), 2);
        assert_eq!(m[1].label, "Glasses, dark");
    }

    #[test]
    fn model_json() {
        let m: Vec<FaceModel> = serde_json::from_str(
            r#"[{"time": 1700000000, "data": [[0.1]], "id": 0, "label": "Initial model"}]"#,
        )
        .unwrap();
        assert_eq!(m[0].label, "Initial model");
    }
}
