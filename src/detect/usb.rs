//! USB device enumeration straight from sysfs (no libusb needed).

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct UsbDevice {
    pub vendor: u16,
    pub product: u16,
    pub manufacturer: Option<String>,
    pub product_name: Option<String>,
}

impl UsbDevice {
    pub fn id_string(&self) -> String {
        format!("{:04x}:{:04x}", self.vendor, self.product)
    }

    pub fn display_name(&self) -> String {
        match (&self.manufacturer, &self.product_name) {
            (Some(m), Some(p)) => format!("{m} {p}"),
            (None, Some(p)) => p.clone(),
            (Some(m), None) => m.clone(),
            (None, None) => format!("USB device {}", self.id_string()),
        }
    }
}

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn read_usb_device(dir: &Path) -> Option<UsbDevice> {
    let vendor = u16::from_str_radix(&read_trim(&dir.join("idVendor"))?, 16).ok()?;
    let product = u16::from_str_radix(&read_trim(&dir.join("idProduct"))?, 16).ok()?;
    Some(UsbDevice {
        vendor,
        product,
        manufacturer: read_trim(&dir.join("manufacturer")),
        product_name: read_trim(&dir.join("product")),
    })
}

pub fn scan() -> Vec<UsbDevice> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/bus/usb/devices") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // "1-2:1.0" style entries are interfaces, "usb1" are root hubs.
        if name.contains(':') || name.starts_with("usb") {
            continue;
        }
        if let Some(dev) = read_usb_device(&entry.path()) {
            out.push(dev);
        }
    }
    out.sort_by_key(|d| (d.vendor, d.product));
    out.dedup_by_key(|d| (d.vendor, d.product));
    out
}
