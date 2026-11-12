//! Hardware + software probe. Blocking; the UI calls it from a worker thread.

pub mod camera;
pub mod hwdb;
pub mod system;
pub mod usb;

use crate::backends::{self, howdy::HowdyInfo, pam::PamState, AuthDevice};
use crate::config::Settings;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Everything we query with dpkg (backend packages plus python-validity's deps).
const QUERY_PACKAGES: &[&str] = &[
    "fprintd",
    "libpam-fprintd",
    "open-fprintd",
    "fprintd-clients",
    "python3-validity",
    "howdy",
    "python3-dbus",
    "python3-usb",
    "python3-yaml",
    "python3-cryptography",
    "innoextract",
];

#[derive(Debug, Clone, Serialize)]
pub struct SystemState {
    pub packages: BTreeMap<String, String>,
    pub units: BTreeMap<String, system::UnitState>,
    pub pam: PamState,
    pub howdy: HowdyInfo,
    pub howdy_installed: bool,
    /// Checkout the running python3-validity unit points at, if any.
    pub validity_checkout_running: Option<PathBuf>,
    /// fprintd's view of readers, or the error talking to it.
    pub fprintd_devices: Result<Vec<(String, String)>, String>,
    pub has_pkexec: bool,
    pub has_apt: bool,
    pub current_user: String,
}

impl SystemState {
    pub fn installed(&self, pkg: &str) -> bool {
        self.packages.contains_key(pkg)
    }
    pub fn unit(&self, name: &str) -> system::UnitState {
        self.units.get(name).cloned().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub devices: Vec<AuthDevice>,
    pub cameras: Vec<camera::Camera>,
    pub usb: Vec<usb::UsbDevice>,
    pub system: SystemState,
}

pub fn probe_system() -> SystemState {
    let units = system::unit_states(system::UNITS);
    let validity_checkout_running = units
        .get("python3-validity.service")
        .filter(|u| u.exists())
        .and_then(|u| system::validity_checkout_from_unit(&u.exec_start));
    let packages = system::installed_packages(QUERY_PACKAGES);
    let howdy_installed = packages.contains_key("howdy") || system::which("howdy").is_some();
    SystemState {
        howdy_installed,
        packages,
        units,
        pam: backends::pam::read(),
        howdy: backends::howdy::info(),
        validity_checkout_running,
        fprintd_devices: backends::fprintd::list_devices_blocking().map_err(|e| e.to_string()),
        has_pkexec: system::which("pkexec").is_some(),
        has_apt: system::which("apt-get").is_some(),
        current_user: system::current_user(),
    }
}

pub fn run(settings: &Settings) -> Report {
    let usb = usb::scan();
    let cameras = camera::scan();
    let sys = probe_system();
    let devices = backends::evaluate(&usb, &cameras, &sys, settings);
    Report {
        devices,
        cameras,
        usb,
        system: sys,
    }
}
