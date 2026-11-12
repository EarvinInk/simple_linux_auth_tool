//! Installed software, services and accounts.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

pub const UNITS: &[&str] = &[
    "fprintd.service",
    "open-fprintd.service",
    "python3-validity.service",
];

#[derive(Debug, Clone, Default, Serialize)]
pub struct UnitState {
    pub load: String,
    pub active: String,
    pub enabled: String,
    pub exec_start: String,
}

impl UnitState {
    pub fn exists(&self) -> bool {
        self.load == "loaded"
    }
    pub fn is_active(&self) -> bool {
        self.active == "active"
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub full_name: String,
}

/// Installed package name -> version.
pub fn installed_packages(names: &[&str]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Ok(out) = Command::new("dpkg-query")
        .args(["-W", "-f=${Package}\t${db:Status-Abbrev}\t${Version}\n"])
        .args(names)
        .output()
    else {
        return map;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.split('\t');
        if let (Some(pkg), Some(status), Some(ver)) = (parts.next(), parts.next(), parts.next()) {
            // "ii " = installed; "rc " = removed with config left.
            if status.starts_with("ii") {
                map.insert(pkg.to_string(), ver.to_string());
            }
        }
    }
    map
}

pub fn unit_states(units: &[&str]) -> BTreeMap<String, UnitState> {
    let mut map = BTreeMap::new();
    let Ok(out) = Command::new("systemctl")
        .arg("show")
        .args(units)
        .args(["-p", "Id,LoadState,ActiveState,UnitFileState,ExecStart"])
        .output()
    else {
        return map;
    };
    parse_systemctl_show(&String::from_utf8_lossy(&out.stdout), units, &mut map);
    map
}

pub fn parse_systemctl_show(text: &str, units: &[&str], map: &mut BTreeMap<String, UnitState>) {
    // One block per unit, in argument order, separated by blank lines.
    for (block, unit) in text.split("\n\n").zip(units.iter()) {
        let mut st = UnitState::default();
        for line in block.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k {
                "LoadState" => st.load = v.to_string(),
                "ActiveState" => st.active = v.to_string(),
                "UnitFileState" => st.enabled = v.to_string(),
                "ExecStart" => {
                    // "{ path=/usr/bin/python3 ; argv[]=/usr/bin/python3 /x/dbus-service ; ... }"
                    if let Some(rest) = v.split("argv[]=").nth(1) {
                        st.exec_start = rest.split(" ;").next().unwrap_or("").trim().to_string();
                    }
                }
                _ => {}
            }
        }
        map.insert(unit.to_string(), st);
    }
}

/// If python3-validity runs from a source checkout, return that checkout.
pub fn validity_checkout_from_unit(exec_start: &str) -> Option<PathBuf> {
    exec_start
        .split_whitespace()
        .find(|a| a.ends_with("/dbus_service/dbus-service"))
        .map(|a| PathBuf::from(a.trim_end_matches("/dbus_service/dbus-service")))
}

pub fn is_validity_checkout(path: &std::path::Path) -> bool {
    path.join("dbus_service/dbus-service").is_file() && path.join("validitysensor").is_dir()
}

pub fn which(cmd: &str) -> Option<PathBuf> {
    [
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
    ]
    .iter()
    .map(|d| PathBuf::from(d).join(cmd))
    .find(|p| p.is_file())
}

/// Human accounts (uid >= 1000) from /etc/passwd.
pub fn human_users() -> Vec<User> {
    let text = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    parse_passwd(&text)
}

pub fn parse_passwd(text: &str) -> Vec<User> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 7 {
                return None;
            }
            let uid: u32 = f[2].parse().ok()?;
            let shell = f[6];
            if !(1000..60000).contains(&uid)
                || shell.ends_with("nologin")
                || shell.ends_with("false")
            {
                return None;
            }
            Some(User {
                name: f[0].to_string(),
                uid,
                full_name: f[4].split(',').next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

pub fn current_user() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| {
            let out = Command::new("id").arg("-un").output().ok()?;
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemctl_show_blocks() {
        let text = "Id=fprintd.service\nLoadState=not-found\nActiveState=inactive\nUnitFileState=\n\nId=python3-validity.service\nLoadState=loaded\nActiveState=active\nUnitFileState=enabled\nExecStart={ path=/usr/bin/python3 ; argv[]=/usr/bin/python3 /home/u/python-validity/dbus_service/dbus-service ; ignore_errors=no }\n";
        let mut map = BTreeMap::new();
        parse_systemctl_show(
            text,
            &["fprintd.service", "python3-validity.service"],
            &mut map,
        );
        assert!(!map["fprintd.service"].exists());
        let v = &map["python3-validity.service"];
        assert!(v.exists() && v.is_active());
        assert_eq!(
            validity_checkout_from_unit(&v.exec_start),
            Some(PathBuf::from("/home/u/python-validity"))
        );
    }

    #[test]
    fn passwd_filters_system_accounts() {
        let users = parse_passwd(
            "root:x:0:0:root:/root:/bin/bash\nnobody:x:65534:65534::/:/usr/sbin/nologin\nkev:x:1000:1000:Kev K,,,:/home/kev:/bin/bash\n",
        );
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].name, "kev");
        assert_eq!(users[0].full_name, "Kev K");
    }
}
