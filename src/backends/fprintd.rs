//! fprintd D-Bus client (works with both fprintd and open-fprintd).
//!
//! The async functions are driven directly from the GTK main loop; zbus runs its
//! own internal executor thread, so no extra runtime is needed.

use anyhow::{anyhow, Result};
use futures_util::{future::Either, StreamExt};
use zbus::zvariant::OwnedObjectPath;

const BUS_NAME: &str = "net.reactivated.Fprint";
const MANAGER_PATH: &str = "/net/reactivated/Fprint/Manager";
const MANAGER_IFACE: &str = "net.reactivated.Fprint.Manager";
const DEVICE_IFACE: &str = "net.reactivated.Fprint.Device";

pub const FINGERS: [(&str, &str); 10] = [
    ("left-thumb", "Thumb"),
    ("left-index-finger", "Index"),
    ("left-middle-finger", "Middle"),
    ("left-ring-finger", "Ring"),
    ("left-little-finger", "Little"),
    ("right-thumb", "Thumb"),
    ("right-index-finger", "Index"),
    ("right-middle-finger", "Middle"),
    ("right-ring-finger", "Ring"),
    ("right-little-finger", "Little"),
];

pub fn finger_label(id: &str) -> String {
    let hand = if id.starts_with("left") {
        "Left"
    } else {
        "Right"
    };
    let name = FINGERS
        .iter()
        .find(|(f, _)| *f == id)
        .map(|(_, n)| *n)
        .unwrap_or(id);
    format!("{hand} {}", name.to_lowercase())
}

fn is_dbus_error(e: &zbus::Error, suffix: &str) -> bool {
    matches!(e, zbus::Error::MethodError(name, _, _) if name.as_str().ends_with(suffix))
}

fn friendly(e: zbus::Error) -> anyhow::Error {
    match &e {
        zbus::Error::MethodError(name, Some(desc), _) => anyhow!("{desc} ({})", name.as_str()),
        zbus::Error::MethodError(name, None, _) => anyhow!("{}", name.as_str()),
        _ => anyhow!(e),
    }
}

// ---------------------------------------------------------------- blocking

/// Devices fprintd currently exposes, as (object path, name). Used by detection.
pub fn list_devices_blocking() -> Result<Vec<(String, String)>> {
    let conn = zbus::blocking::Connection::system()?;
    let mgr = zbus::blocking::Proxy::new(&conn, BUS_NAME, MANAGER_PATH, MANAGER_IFACE)?;
    let paths: Vec<OwnedObjectPath> = mgr.call("GetDevices", &()).map_err(friendly)?;
    let mut out = Vec::new();
    for p in paths {
        let dev = zbus::blocking::Proxy::new(&conn, BUS_NAME, p.as_str().to_owned(), DEVICE_IFACE)?;
        let name: String = dev
            .get_property("name")
            .unwrap_or_else(|_| "Fingerprint reader".into());
        out.push((p.as_str().to_owned(), name));
    }
    Ok(out)
}

// ---------------------------------------------------------------- async

pub struct Device {
    proxy: zbus::Proxy<'static>,
    pub stages: Option<u32>,
}

impl Device {
    pub async fn default() -> Result<Self> {
        let conn = zbus::Connection::system().await?;
        let mgr = zbus::Proxy::new(&conn, BUS_NAME, MANAGER_PATH, MANAGER_IFACE).await?;
        let path: OwnedObjectPath = match mgr.call("GetDefaultDevice", &()).await {
            Ok(p) => p,
            Err(e) if is_dbus_error(&e, "NoSuchDevice") => {
                return Err(anyhow!(
                    "fprintd does not see a fingerprint reader. Is the driver service running?"
                ))
            }
            Err(e) => return Err(friendly(e)),
        };
        let proxy =
            zbus::Proxy::new(&conn, BUS_NAME, path.as_str().to_owned(), DEVICE_IFACE).await?;
        let stages = proxy
            .get_property::<i32>("num-enroll-stages")
            .await
            .ok()
            .filter(|s| *s > 0)
            .map(|s| s as u32);
        Ok(Device { proxy, stages })
    }

    pub async fn enrolled(&self, user: &str) -> Result<Vec<String>> {
        match self
            .proxy
            .call::<_, _, Vec<String>>("ListEnrolledFingers", &(user,))
            .await
        {
            Ok(v) => Ok(v),
            Err(e) if is_dbus_error(&e, "NoEnrolledPrints") => Ok(Vec::new()),
            Err(e) => Err(friendly(e)),
        }
    }

    async fn claim(&self, user: &str) -> Result<()> {
        self.proxy
            .call_method("Claim", &(user,))
            .await
            .map_err(friendly)?;
        Ok(())
    }

    async fn release(&self) {
        let _ = self.proxy.call_method("Release", &()).await;
    }

    /// Delete one finger. open-fprintd lacks DeleteEnrolledFinger; that case
    /// returns `Ok(false)` so the UI can offer "delete all" instead.
    pub async fn delete_finger(&self, user: &str, finger: &str) -> Result<bool> {
        self.claim(user).await?;
        let res = self
            .proxy
            .call_method("DeleteEnrolledFinger", &(finger,))
            .await;
        self.release().await;
        match res {
            Ok(_) => Ok(true),
            Err(e) if is_dbus_error(&e, "UnknownMethod") => Ok(false),
            Err(e) => Err(friendly(e)),
        }
    }

    pub async fn delete_all(&self, user: &str) -> Result<()> {
        self.claim(user).await?;
        let res = self.proxy.call_method("DeleteEnrolledFingers2", &()).await;
        self.release().await;
        match res {
            Ok(_) => Ok(()),
            Err(e) if is_dbus_error(&e, "UnknownMethod") => {
                // Very old fprintd: unclaimed variant taking the user name.
                self.proxy
                    .call_method("DeleteEnrolledFingers", &(user,))
                    .await
                    .map_err(friendly)?;
                Ok(())
            }
            Err(e) => Err(friendly(e)),
        }
    }

    /// Run an enroll or verify session. `on_status(result, done)` is called for
    /// every status signal. Returns the final status string, or "cancelled".
    async fn session(
        &self,
        user: &str,
        start: (&str, &str),
        stop: &str,
        signal: &str,
        cancel: async_channel::Receiver<()>,
        mut on_status: impl FnMut(&str, bool),
    ) -> Result<String> {
        self.claim(user).await?;
        let result = async {
            let mut stream = Box::pin(self.proxy.receive_signal(signal).await?);
            self.proxy
                .call_method(start.0, &(start.1,))
                .await
                .map_err(friendly)?;
            loop {
                let next = stream.next();
                let cancelled = Box::pin(cancel.recv());
                match futures_util::future::select(next, cancelled).await {
                    Either::Left((Some(msg), _)) => {
                        let (status, done): (String, bool) = msg.body().deserialize()?;
                        on_status(&status, done);
                        if done {
                            return Ok::<_, anyhow::Error>(status);
                        }
                    }
                    Either::Left((None, _)) => return Err(anyhow!("fprintd went away")),
                    Either::Right(_) => return Ok("cancelled".to_string()),
                }
            }
        }
        .await;
        let _ = self.proxy.call_method(stop, &()).await;
        self.release().await;
        result
    }

    pub async fn enroll(
        &self,
        user: &str,
        finger: &str,
        cancel: async_channel::Receiver<()>,
        on_status: impl FnMut(&str, bool),
    ) -> Result<String> {
        self.session(
            user,
            ("EnrollStart", finger),
            "EnrollStop",
            "EnrollStatus",
            cancel,
            on_status,
        )
        .await
    }

    pub async fn verify(
        &self,
        user: &str,
        cancel: async_channel::Receiver<()>,
        on_status: impl FnMut(&str, bool),
    ) -> Result<String> {
        self.session(
            user,
            ("VerifyStart", "any"),
            "VerifyStop",
            "VerifyStatus",
            cancel,
            on_status,
        )
        .await
    }
}

/// Human text for EnrollStatus / VerifyStatus results.
pub fn status_message(status: &str) -> &'static str {
    match status {
        "enroll-stage-passed" => "Good. Lift your finger and place it again",
        "enroll-completed" => "Fingerprint enrolled",
        "enroll-failed" => "Enrollment failed",
        "enroll-retry-scan" | "verify-retry-scan" => "Couldn't read that — try again",
        "enroll-swipe-too-short" | "verify-swipe-too-short" => "Swipe was too short — try again",
        "enroll-finger-not-centered" | "verify-finger-not-centered" => {
            "Center your finger on the sensor"
        }
        "enroll-remove-and-retry" | "verify-remove-and-retry" => "Remove your finger and try again",
        "enroll-data-full" => "The sensor's storage is full",
        "enroll-duplicate" => "This finger is already enrolled",
        "enroll-disconnected" | "verify-disconnected" => "The reader was disconnected",
        "verify-match" => "Match — fingerprint recognised",
        "verify-no-match" => "No match",
        "cancelled" => "Cancelled",
        _ => "Unexpected error from the reader",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(finger_label("right-index-finger"), "Right index");
        assert_eq!(finger_label("left-thumb"), "Left thumb");
    }
}
