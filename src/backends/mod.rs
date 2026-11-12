//! Decide, per detected device, which backend it needs, whether that backend
//! is ready, and which privileged steps would get it there.

pub mod fprintd;
pub mod howdy;
pub mod pam;

use crate::config::{known_devices, KnownBackend, KnownDevice, Settings, ValiditySource};
use crate::detect::{camera::Camera, hwdb, system, usb::UsbDevice, SystemState};
use crate::helper::Action;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Method {
    Fingerprint,
    Face,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Backend {
    Libfprint {
        driver: Option<String>,
    },
    PythonValidity {
        source: ValiditySource,
        checkout: Option<PathBuf>,
    },
    Howdy,
    None,
}

impl Backend {
    pub fn label(&self) -> String {
        match self {
            Backend::Libfprint { driver: Some(d) } => format!("fprintd (libfprint “{d}” driver)"),
            Backend::Libfprint { driver: None } => "fprintd (libfprint)".into(),
            Backend::PythonValidity {
                source: ValiditySource::Ppa,
                ..
            } => "open-fprintd + python-validity".into(),
            Backend::PythonValidity {
                source: ValiditySource::Local,
                ..
            } => "open-fprintd + python-validity (source checkout)".into(),
            Backend::Howdy => "Howdy".into(),
            Backend::None => "No Linux driver known".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    Ready,
    NeedsSetup,
    Conflict,
    Unsupported,
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Ready => "Ready",
            Status::NeedsSetup => "Needs setup",
            Status::Conflict => "Conflict",
            Status::Unsupported => "Unsupported",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthDevice {
    /// Stable key, e.g. "usb:06cb:00b7" or "face".
    pub key: String,
    pub method: Method,
    pub title: String,
    pub hardware: String,
    pub backend: Backend,
    pub status: Status,
    /// Things worth telling the user (warnings, explanations).
    pub notes: Vec<String>,
    /// Privileged steps that bring the device to Ready.
    pub setup: Vec<Action>,
    /// Why `setup` cannot run yet (e.g. a checkout must be chosen first).
    pub setup_blocker: Option<String>,
    /// Face only: the camera Howdy uses (or should use).
    pub camera: Option<Camera>,
}

pub fn evaluate(
    usb: &[UsbDevice],
    cameras: &[Camera],
    sys: &SystemState,
    settings: &Settings,
) -> Vec<AuthDevice> {
    let known = known_devices();
    let hw = hwdb::load();
    let mut out = Vec::new();

    for dev in usb {
        let k = known
            .iter()
            .find(|k| k.usb_id() == Some((dev.vendor, dev.product)));
        let h = hw.get(&(dev.vendor, dev.product));
        let named_fp = dev.display_name().to_lowercase().contains("fingerprint");
        if k.is_none() && h.is_none() && !named_fp {
            continue;
        }
        out.push(evaluate_fingerprint(dev, k, h, sys, settings));
    }

    // A reader fprintd knows about but we did not see on USB (SPI/I²C sensors).
    if !out.iter().any(|d| d.method == Method::Fingerprint) {
        if let Ok(devs) = &sys.fprintd_devices {
            if let Some((_, name)) = devs.first() {
                out.push(AuthDevice {
                    key: "fprintd".into(),
                    method: Method::Fingerprint,
                    title: name.clone(),
                    hardware: "Reported by fprintd".into(),
                    backend: Backend::Libfprint { driver: None },
                    status: Status::Ready,
                    notes: vec![],
                    setup: vec![],
                    setup_blocker: None,
                    camera: None,
                });
            }
        }
    }

    if let Some(face) = evaluate_face(cameras, sys) {
        out.push(face);
    }
    out
}

fn missing(sys: &SystemState, pkgs: &[&str]) -> Vec<String> {
    pkgs.iter()
        .filter(|p| !sys.installed(p))
        .map(|p| p.to_string())
        .collect()
}

fn installed_of(sys: &SystemState, pkgs: &[&str]) -> Vec<String> {
    pkgs.iter()
        .filter(|p| sys.installed(p))
        .map(|p| p.to_string())
        .collect()
}

fn install_steps(ppa: Option<&str>, packages: Vec<String>) -> Vec<Action> {
    let mut steps = Vec::new();
    if packages.is_empty() {
        return steps;
    }
    if let Some(ppa) = ppa {
        steps.push(Action::AddPpa { ppa: ppa.into() });
    }
    steps.push(Action::AptUpdate);
    steps.push(Action::AptInstall { packages });
    steps
}

fn evaluate_fingerprint(
    dev: &UsbDevice,
    known: Option<&KnownDevice>,
    hw: Option<&hwdb::HwdbEntry>,
    sys: &SystemState,
    settings: &Settings,
) -> AuthDevice {
    let mut d = AuthDevice {
        key: format!("usb:{}", dev.id_string()),
        method: Method::Fingerprint,
        title: known
            .map(|k| k.name.clone())
            .unwrap_or_else(|| dev.display_name()),
        hardware: if dev.product_name.is_some() || dev.manufacturer.is_some() {
            format!("{} · USB {}", dev.display_name(), dev.id_string())
        } else {
            format!("USB {}", dev.id_string())
        },
        backend: Backend::None,
        status: Status::Unsupported,
        notes: known.and_then(|k| k.notes.clone()).into_iter().collect(),
        setup: vec![],
        setup_blocker: None,
        camera: None,
    };

    let backend = match (known.map(|k| k.backend), hw) {
        (Some(KnownBackend::PythonValidity), _) => {
            let source = known
                .and_then(|k| k.validity)
                .unwrap_or(ValiditySource::Ppa);
            let checkout = sys
                .validity_checkout_running
                .clone()
                .or_else(|| settings.validity_checkout.clone());
            Backend::PythonValidity { source, checkout }
        }
        (Some(KnownBackend::Libfprint), h)
        | (None, h @ Some(hwdb::HwdbEntry::Supported { .. })) => {
            let driver = match h {
                Some(hwdb::HwdbEntry::Supported { driver }) => Some(driver.clone()),
                _ => None,
            };
            Backend::Libfprint { driver }
        }
        (Some(KnownBackend::Unsupported), _) | (None, Some(hwdb::HwdbEntry::Unsupported)) => {
            Backend::None
        }
        (None, None) => Backend::Libfprint { driver: None },
    };
    d.backend = backend.clone();

    match backend {
        Backend::None => {
            d.notes.push(
                "libfprint lists this sensor as unsupported. Check whether a python-validity fork or a vendor \
                 libfprint-TOD driver exists for it; you can describe it in ~/.config/simple-auth-tool/devices.toml."
                    .into(),
            );
        }
        Backend::Libfprint { .. } => {
            let conflicting = installed_of(sys, &["open-fprintd", "python3-validity"]);
            let need = missing(sys, &["fprintd", "libpam-fprintd"]);
            if !conflicting.is_empty() {
                d.status = Status::Conflict;
                d.notes.push(format!(
                    "{} replaces stock fprintd, but this reader is driven by libfprint and needs fprintd.",
                    conflicting.join(" and ")
                ));
                d.setup.push(Action::AptRemove {
                    packages: conflicting,
                });
                d.setup.extend(install_steps(
                    None,
                    vec!["fprintd".into(), "libpam-fprintd".into()],
                ));
            } else if !need.is_empty() {
                d.status = Status::NeedsSetup;
                d.setup = install_steps(None, need);
            } else {
                d.status = Status::Ready;
            }
        }
        Backend::PythonValidity {
            source,
            ref checkout,
        } => {
            let stock = installed_of(sys, &["fprintd", "libpam-fprintd"]);
            if !stock.is_empty() {
                d.notes.push("Stock fprintd is installed; it conflicts with open-fprintd and will be removed.".into());
                d.setup.push(Action::AptRemove { packages: stock });
            }
            let unit = sys.unit("python3-validity.service");
            match source {
                ValiditySource::Ppa => {
                    let need = missing(
                        sys,
                        &["open-fprintd", "fprintd-clients", "python3-validity"],
                    );
                    if need
                        .iter()
                        .any(|p| p == "open-fprintd" || p == "python3-validity")
                    {
                        d.setup
                            .extend(install_steps(Some("ppa:uunicorn/open-fprintd"), need));
                    } else {
                        d.setup.extend(install_steps(None, need));
                    }
                }
                ValiditySource::Local => {
                    let running_from_checkout = unit.is_active()
                        && sys.validity_checkout_running.is_some()
                        && sys.validity_checkout_running == *checkout;
                    // A working service already has its Python deps (maybe from pip);
                    // only a fresh install needs the distro packages.
                    let wanted: &[&str] = if running_from_checkout {
                        &["open-fprintd", "fprintd-clients"]
                    } else {
                        &[
                            "open-fprintd",
                            "fprintd-clients",
                            "python3-dbus",
                            "python3-usb",
                            "python3-yaml",
                            "python3-cryptography",
                            "innoextract",
                        ]
                    };
                    let mut need = missing(sys, wanted);
                    let ppa = need
                        .iter()
                        .any(|p| p == "open-fprintd")
                        .then_some("ppa:uunicorn/open-fprintd");
                    need.sort();
                    d.setup.extend(install_steps(ppa, need));
                    // The unit exists and already points at this checkout (it may just be stopped).
                    let unit_points_here = unit.exists()
                        && sys.validity_checkout_running.is_some()
                        && sys.validity_checkout_running == *checkout;
                    match checkout {
                        Some(c) if system::is_validity_checkout(c) => {
                            if !unit_points_here {
                                d.setup.push(Action::InstallValidityLocal {
                                    checkout: c.display().to_string(),
                                });
                            }
                        }
                        Some(c) => {
                            d.setup_blocker = Some(format!(
                                "{} is not a python-validity checkout.",
                                c.display()
                            ));
                        }
                        None => {
                            d.setup_blocker =
                                Some("Choose the python-validity checkout (a fork that supports this sensor).".into());
                        }
                    }
                }
            }
            if d.setup.is_empty() && d.setup_blocker.is_none() && !unit.is_active() {
                d.notes
                    .push("The python3-validity service is not running.".into());
                d.setup.push(Action::Systemctl {
                    verb: "restart".into(),
                    unit: "python3-validity.service".into(),
                });
                d.setup.push(Action::Systemctl {
                    verb: "restart".into(),
                    unit: "open-fprintd.service".into(),
                });
            }
            d.status = if d.setup.is_empty() && d.setup_blocker.is_none() {
                Status::Ready
            } else {
                Status::NeedsSetup
            };
            if let Some(c) = checkout
                .as_ref()
                .filter(|_| source == ValiditySource::Local)
            {
                d.notes.push(format!("Driver source: {}", c.display()));
            }
        }
        Backend::Howdy => unreachable!(),
    }

    if d.status == Status::Ready {
        match &sys.fprintd_devices {
            Ok(list) if list.is_empty() => {
                d.notes.push("The driver is installed but fprintd does not see the reader yet. Try “Restart services”.".into())
            }
            Err(e) => d.notes.push(format!("Could not talk to fprintd: {e}")),
            _ => {}
        }
    }
    d
}

fn evaluate_face(cameras: &[Camera], sys: &SystemState) -> Option<AuthDevice> {
    if cameras.is_empty() {
        return None;
    }
    let configured = sys.howdy.device_path.as_ref().and_then(|p| {
        let real = std::fs::canonicalize(p).ok();
        cameras
            .iter()
            .find(|c| &c.node == p || real.as_deref() == Some(std::path::Path::new(&c.node)))
    });
    let best_ir = cameras.iter().find(|c| c.is_ir);
    let camera = configured.or(best_ir).or(cameras.first()).cloned()?;

    let mut d = AuthDevice {
        key: "face".into(),
        method: Method::Face,
        title: "Face unlock".into(),
        hardware: format!("{} · {}", camera.name, camera.node),
        backend: Backend::Howdy,
        status: Status::Ready,
        notes: vec![],
        setup: vec![],
        setup_blocker: None,
        camera: Some(camera.clone()),
    };

    if best_ir.is_none() {
        d.notes.push(
            "No infrared camera found. Howdy works with a normal webcam, but then a photo of your face can unlock the \
             computer. Only use it for convenience."
                .into(),
        );
    }

    let user = sys.current_user.clone();
    let set_camera = |node: &str| Action::HowdySet {
        user: user.clone(),
        key: "device_path".into(),
        value: node.into(),
    };
    let preferred = best_ir.unwrap_or(&camera).node.clone();

    if !sys.howdy_installed {
        d.status = Status::NeedsSetup;
        d.setup = install_steps(Some("ppa:boltgolt/howdy"), vec!["howdy".into()]);
        d.setup.push(set_camera(&preferred));
        d.notes.push(
            "Howdy comes from ppa:boltgolt/howdy. If that PPA has no build for your release, install Howdy another \
             way and press Refresh."
                .into(),
        );
        return Some(d);
    }

    match (&sys.howdy.device_path, configured) {
        (None, _) => {
            d.status = Status::NeedsSetup;
            d.notes.push("Howdy has no camera configured.".into());
            d.setup.push(set_camera(&preferred));
        }
        (Some(p), None) => {
            d.status = Status::NeedsSetup;
            d.notes
                .push(format!("Howdy is set to {p}, which does not exist."));
            d.setup.push(set_camera(&preferred));
        }
        (Some(_), Some(c)) if !c.is_ir && best_ir.is_some() => {
            d.notes.push(format!(
                "Howdy uses the regular camera {}; the IR camera {} is safer.",
                c.node,
                best_ir.unwrap().node
            ));
        }
        _ => {}
    }
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::system::UnitState;
    use std::collections::BTreeMap;

    fn sys(pkgs: &[&str]) -> SystemState {
        SystemState {
            packages: pkgs.iter().map(|p| (p.to_string(), "1".into())).collect(),
            units: BTreeMap::new(),
            pam: pam::PamState {
                fprintd: pam::PamMethod {
                    profile: "fprintd",
                    profile_installed: true,
                    common_auth: pam::LineState::Absent,
                },
                howdy: pam::PamMethod {
                    profile: "howdy",
                    profile_installed: true,
                    common_auth: pam::LineState::Absent,
                },
            },
            howdy: howdy::HowdyInfo {
                config_path: None,
                device_path: None,
                compare_py: None,
            },
            howdy_installed: pkgs.contains(&"howdy"),
            validity_checkout_running: None,
            fprintd_devices: Ok(vec![("/x".into(), "r".into())]),
            has_pkexec: true,
            has_apt: true,
            current_user: "kev".into(),
        }
    }

    fn usb(v: u16, p: u16) -> UsbDevice {
        UsbDevice {
            vendor: v,
            product: p,
            manufacturer: None,
            product_name: Some("Fingerprint reader".into()),
        }
    }

    #[test]
    fn libfprint_device_needs_fprintd() {
        let s = sys(&[]);
        let hw = hwdb::HwdbEntry::Supported {
            driver: "synaptics".into(),
        };
        let d = evaluate_fingerprint(
            &usb(0x06cb, 0x00bd),
            None,
            Some(&hw),
            &s,
            &Settings::default(),
        );
        assert_eq!(d.status, Status::NeedsSetup);
        assert!(d.setup.contains(&Action::AptInstall {
            packages: vec!["fprintd".into(), "libpam-fprintd".into()]
        }));
    }

    #[test]
    fn libfprint_conflicts_with_open_fprintd() {
        let s = sys(&["open-fprintd", "fprintd-clients"]);
        let hw = hwdb::HwdbEntry::Supported {
            driver: "goodixmoc".into(),
        };
        let d = evaluate_fingerprint(
            &usb(0x27c6, 0x6094),
            None,
            Some(&hw),
            &s,
            &Settings::default(),
        );
        assert_eq!(d.status, Status::Conflict);
        assert_eq!(
            d.setup[0],
            Action::AptRemove {
                packages: vec!["open-fprintd".into()]
            }
        );
    }

    #[test]
    fn local_validity_without_checkout_is_blocked() {
        let s = sys(&[]);
        let k = crate::config::parse_devices(crate::config::BUILTIN_DEVICES).unwrap();
        let k = k.iter().find(|k| k.usb == "06cb:00b7").unwrap();
        let d = evaluate_fingerprint(
            &usb(0x06cb, 0x00b7),
            Some(k),
            Some(&hwdb::HwdbEntry::Unsupported),
            &s,
            &Settings::default(),
        );
        assert_eq!(d.status, Status::NeedsSetup);
        assert!(d.setup_blocker.is_some());
        assert!(d.setup.contains(&Action::AddPpa {
            ppa: "ppa:uunicorn/open-fprintd".into()
        }));
    }

    #[test]
    fn local_validity_running_is_ready() {
        let mut s = sys(&[
            "open-fprintd",
            "fprintd-clients",
            "python3-dbus",
            "python3-usb",
            "python3-yaml",
            "python3-cryptography",
            "innoextract",
        ]);
        let dir = std::env::temp_dir().join(format!("sat-validity-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("dbus_service")).unwrap();
        std::fs::create_dir_all(dir.join("validitysensor")).unwrap();
        std::fs::write(dir.join("dbus_service/dbus-service"), "").unwrap();
        s.validity_checkout_running = Some(dir.clone());
        s.units.insert(
            "python3-validity.service".into(),
            UnitState {
                load: "loaded".into(),
                active: "active".into(),
                ..Default::default()
            },
        );
        let k = crate::config::parse_devices(crate::config::BUILTIN_DEVICES).unwrap();
        let k = k.iter().find(|k| k.usb == "06cb:00b7").unwrap();
        let d = evaluate_fingerprint(
            &usb(0x06cb, 0x00b7),
            Some(k),
            None,
            &s,
            &Settings::default(),
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(d.status, Status::Ready, "{d:?}");
        assert!(d.setup.is_empty());
    }

    #[test]
    fn face_prefers_ir_camera() {
        let cams = vec![
            Camera {
                node: "/dev/video0".into(),
                name: "HD Camera".into(),
                is_ir: false,
                usb_id: None,
            },
            Camera {
                node: "/dev/video2".into(),
                name: "IR Camera".into(),
                is_ir: true,
                usb_id: None,
            },
        ];
        let d = evaluate_face(&cams, &sys(&[])).unwrap();
        assert_eq!(d.status, Status::NeedsSetup);
        assert_eq!(d.camera.unwrap().node, "/dev/video2");
        assert!(d.setup.contains(&Action::HowdySet {
            user: "kev".into(),
            key: "device_path".into(),
            value: "/dev/video2".into()
        }));
    }
}
