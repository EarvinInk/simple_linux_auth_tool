//! The privileged side. The UI runs `pkexec <this binary> --helper '<json>'`,
//! where the JSON is a list of [`Action`]s. Every action is checked against a
//! fixed allowlist before anything runs, commands are executed without a
//! shell, and the run stops at the first failure.
//!
//! Output protocol on stdout (everything else is passed through verbatim):
//!   `##STEP <n>/<total> <description>`
//!   `##FAILED <n> <exit code>`
//!   `##DONE`

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub const INSTALLABLE: &[&str] = &[
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
    "v4l-utils",
    "software-properties-common",
];
pub const REMOVABLE: &[&str] = &[
    "fprintd",
    "libpam-fprintd",
    "open-fprintd",
    "python3-validity",
];
pub const PPAS: &[&str] = &["ppa:uunicorn/open-fprintd", "ppa:boltgolt/howdy"];
pub const UNITS: &[&str] = &[
    "fprintd.service",
    "open-fprintd.service",
    "open-fprintd-resume.service",
    "open-fprintd-suspend.service",
    "python3-validity.service",
];
pub const VERBS: &[&str] = &[
    "start", "stop", "restart", "enable", "disable", "mask", "unmask",
];
pub const PAM_PROFILES: &[&str] = &["fprintd", "howdy"];

const VALIDITY_UNIT_PATH: &str = "/etc/systemd/system/python3-validity.service";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Action {
    AptUpdate,
    AptInstall {
        packages: Vec<String>,
    },
    AptRemove {
        packages: Vec<String>,
    },
    AddPpa {
        ppa: String,
    },
    Systemctl {
        verb: String,
        unit: String,
    },
    PamAuthUpdate {
        profile: String,
        enable: bool,
        force: bool,
    },
    HowdySet {
        user: String,
        key: String,
        value: String,
    },
    HowdyAdd {
        user: String,
        label: String,
    },
    HowdyRemove {
        user: String,
        id: u32,
    },
    HowdyClear {
        user: String,
    },
    HowdyList {
        user: String,
    },
    HowdyTest {
        user: String,
    },
    InstallValidityLocal {
        checkout: String,
    },
}

impl Action {
    pub fn describe(&self) -> String {
        match self {
            Action::AptUpdate => "Refresh package lists".into(),
            Action::AptInstall { packages } => format!("Install {}", packages.join(", ")),
            Action::AptRemove { packages } => format!("Remove {}", packages.join(", ")),
            Action::AddPpa { ppa } => format!("Add software source {ppa}"),
            Action::Systemctl { verb, unit } => format!("{} {unit}", capitalize(verb)),
            Action::PamAuthUpdate {
                profile,
                enable,
                force,
            } => format!(
                "{} {profile} in PAM{}",
                if *enable { "Enable" } else { "Disable" },
                if *force {
                    " (overwriting local edits)"
                } else {
                    ""
                }
            ),
            Action::HowdySet { key, value, .. } => format!("Set Howdy {key} = {value}"),
            Action::HowdyAdd { user, label } => format!("Add face model \"{label}\" for {user}"),
            Action::HowdyRemove { user, id } => format!("Remove face model {id} of {user}"),
            Action::HowdyClear { user } => format!("Remove all face models of {user}"),
            Action::HowdyList { user } => format!("List face models of {user}"),
            Action::HowdyTest { user } => format!("Test face recognition for {user}"),
            Action::InstallValidityLocal { checkout } => {
                format!("Install python-validity service from {checkout}")
            }
        }
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            Action::AptUpdate => Ok(()),
            Action::AptInstall { packages } => check_list(packages, INSTALLABLE, "package"),
            Action::AptRemove { packages } => check_list(packages, REMOVABLE, "package"),
            Action::AddPpa { ppa } => check_one(ppa, PPAS, "software source"),
            Action::Systemctl { verb, unit } => {
                check_one(verb, VERBS, "systemctl verb")?;
                check_one(unit, UNITS, "unit")
            }
            Action::PamAuthUpdate { profile, .. } => {
                check_one(profile, PAM_PROFILES, "PAM profile")
            }
            Action::HowdySet { user, key, value } => {
                check_user(user)?;
                check_howdy_setting(key, value)
            }
            Action::HowdyAdd { user, label } => {
                check_user(user)?;
                check_label(label)
            }
            Action::HowdyRemove { user, .. }
            | Action::HowdyClear { user }
            | Action::HowdyList { user }
            | Action::HowdyTest { user } => check_user(user),
            Action::InstallValidityLocal { checkout } => {
                check_checkout(Path::new(checkout)).map(|_| ())
            }
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

fn check_one(value: &str, allowed: &[&str], what: &str) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        bail!("{what} '{value}' is not allowed")
    }
}

fn check_list(values: &[String], allowed: &[&str], what: &str) -> Result<()> {
    if values.is_empty() {
        bail!("no {what}s given");
    }
    values.iter().try_for_each(|v| check_one(v, allowed, what))
}

pub fn valid_user_name(user: &str) -> bool {
    !user.is_empty()
        && user.len() <= 32
        && user
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && user
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'))
}

fn check_user(user: &str) -> Result<()> {
    if !valid_user_name(user) {
        bail!("invalid user name '{user}'");
    }
    if !crate::detect::system::human_users()
        .iter()
        .any(|u| u.name == user)
    {
        bail!("'{user}' is not a regular user account");
    }
    Ok(())
}

pub fn check_label(label: &str) -> Result<()> {
    if label.trim().is_empty() || label.chars().count() > 24 {
        bail!("label must be 1-24 characters");
    }
    if label.chars().any(|c| c.is_control() || c == ',') || label.starts_with('-') {
        bail!("label contains characters Howdy cannot store");
    }
    Ok(())
}

pub fn check_howdy_setting(key: &str, value: &str) -> Result<()> {
    let ok = match key {
        "device_path" => {
            value
                .strip_prefix("/dev/video")
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
                || value.strip_prefix("/dev/v4l/by-id/").is_some_and(|n| {
                    !n.is_empty()
                        && n.chars()
                            .all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
                })
        }
        "certainty" => value
            .parse::<f32>()
            .is_ok_and(|v| (1.0..=10.0).contains(&v)),
        "timeout" => value.parse::<u32>().is_ok_and(|v| (1..=60).contains(&v)),
        "dark_threshold" => value.parse::<u32>().is_ok_and(|v| v <= 100),
        "detection_notice" | "no_confirmation" => value == "true" || value == "false",
        _ => bail!("Howdy setting '{key}' is not allowed"),
    };
    if ok {
        Ok(())
    } else {
        bail!("invalid value '{value}' for {key}")
    }
}

/// A python-validity checkout must look like one and must not be writable by
/// arbitrary users, because its code is going to run as root.
pub fn check_checkout(path: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute() {
        bail!("checkout path must be absolute");
    }
    let real =
        std::fs::canonicalize(path).with_context(|| format!("cannot open {}", path.display()))?;
    if !crate::detect::system::is_validity_checkout(&real) {
        bail!(
            "{} is not a python-validity checkout (no dbus_service/dbus-service)",
            real.display()
        );
    }
    for p in [
        real.clone(),
        real.join("dbus_service"),
        real.join("validitysensor"),
    ] {
        if std::fs::metadata(&p)?.mode() & 0o002 != 0 {
            bail!(
                "{} is world-writable; refusing to run it as root",
                p.display()
            );
        }
    }
    Ok(real)
}

// ------------------------------------------------------------------ running

pub fn main(args: &[String]) -> ExitCode {
    let result = (|| -> Result<i32> {
        if !is_root() {
            bail!("--helper must run as root (via pkexec)");
        }
        let json = args.first().ok_or_else(|| anyhow!("missing action list"))?;
        let actions: Vec<Action> = serde_json::from_str(json).context("bad action list")?;
        for a in &actions {
            a.validate()?;
        }
        let total = actions.len();
        for (i, a) in actions.iter().enumerate() {
            println!("##STEP {}/{} {}", i + 1, total, a.describe());
            flush();
            let code = run_action(a)?;
            if code != 0 {
                println!("##FAILED {} {}", i + 1, code);
                flush();
                return Ok(code);
            }
        }
        println!("##DONE");
        Ok(0)
    })();
    match result {
        Ok(0) => ExitCode::SUCCESS,
        Ok(c) => ExitCode::from(c.clamp(1, 120) as u8),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn flush() {
    let _ = std::io::stdout().flush();
}

fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .map(|l| l.split_whitespace().nth(2) == Some("0"))
        })
        .unwrap_or(false)
}

fn tool(name: &str) -> Result<PathBuf> {
    crate::detect::system::which(name).ok_or_else(|| anyhow!("{name} is not installed"))
}

fn cmd(name: &str) -> Result<Command> {
    let mut c = Command::new(tool(name)?);
    c.env_clear()
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        )
        .env("LANG", "C.UTF-8")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("PYTHONUNBUFFERED", "1");
    Ok(c)
}

fn status(mut c: Command) -> Result<i32> {
    let st = c
        .status()
        .with_context(|| format!("failed to start {:?}", c.get_program()))?;
    Ok(st.code().unwrap_or(1))
}

fn howdy(user: &str) -> Result<Command> {
    let mut c = cmd("howdy")?;
    c.args(["-U", user]);
    Ok(c)
}

fn run_action(a: &Action) -> Result<i32> {
    match a {
        Action::AptUpdate => {
            let mut c = cmd("apt-get")?;
            c.arg("update");
            status(c)
        }
        Action::AptInstall { packages } => {
            let mut c = cmd("apt-get")?;
            c.args(["install", "-y", "--no-install-recommends"])
                .args(packages);
            status(c)
        }
        Action::AptRemove { packages } => {
            let mut c = cmd("apt-get")?;
            c.args(["remove", "-y"]).args(packages);
            status(c)
        }
        Action::AddPpa { ppa } => {
            let mut c = cmd("add-apt-repository")?;
            c.args(["-y", ppa]);
            status(c)
        }
        Action::Systemctl { verb, unit } => {
            let mut c = cmd("systemctl")?;
            c.args([verb, unit]);
            status(c)
        }
        Action::PamAuthUpdate {
            profile,
            enable,
            force,
        } => {
            let mut c = cmd("pam-auth-update")?;
            c.arg(if *enable { "--enable" } else { "--disable" })
                .arg(profile);
            if *force {
                c.arg("--force");
            }
            status(c)
        }
        Action::HowdySet { user, key, value } => {
            let mut c = howdy(user)?;
            c.args(["set", key, value]);
            status(c)
        }
        Action::HowdyAdd { user, label } => {
            let mut c = howdy(user)?;
            c.args(["-y", "add", label]);
            status(c)
        }
        Action::HowdyRemove { user, id } => {
            let mut c = howdy(user)?;
            c.args(["-y", "remove", &id.to_string()]);
            status(c)
        }
        Action::HowdyClear { user } => {
            let mut c = howdy(user)?;
            c.args(["-y", "clear"]);
            status(c)
        }
        Action::HowdyList { user } => {
            let mut c = howdy(user)?;
            c.args(["--plain", "list"]);
            status(c)
        }
        Action::HowdyTest { user } => {
            let compare = crate::backends::howdy::compare_py()
                .ok_or_else(|| anyhow!("Howdy's compare.py not found"))?;
            let mut c = cmd("python3")?;
            c.arg(&compare).arg(user);
            let code = status(c)?;
            println!(
                "##RESULT {code} {}",
                crate::backends::howdy::compare_exit_message(code)
            );
            Ok(code)
        }
        Action::InstallValidityLocal { checkout } => {
            install_validity_local(&check_checkout(Path::new(checkout))?)
        }
    }
}

fn validity_unit(checkout: &Path) -> String {
    format!(
        "[Unit]\nDescription=python-validity driver dbus service (from {dir})\n\n[Service]\nType=simple\nExecStart=/usr/bin/python3 {dir}/dbus_service/dbus-service\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=multi-user.target\n",
        dir = checkout.display()
    )
}

fn install_validity_local(checkout: &Path) -> Result<i32> {
    // D-Bus policy so the service may own io.github.uunicorn.Fprint.
    let conf_src = checkout.join("dbus_service/io.github.uunicorn.Fprint.conf");
    let conf_dst = Path::new("/usr/share/dbus-1/system.d/io.github.uunicorn.Fprint.conf");
    if conf_src.is_file() && !conf_dst.exists() {
        println!("Installing {}", conf_dst.display());
        std::fs::copy(&conf_src, conf_dst)?;
    }
    // Default service config.
    let yaml_src = checkout.join("etc/python-validity/dbus-service.yaml");
    let yaml_dst = Path::new("/etc/python-validity/dbus-service.yaml");
    if yaml_src.is_file() && !yaml_dst.exists() {
        std::fs::create_dir_all("/etc/python-validity")?;
        std::fs::copy(&yaml_src, yaml_dst)?;
    }
    println!("Writing {VALIDITY_UNIT_PATH}");
    std::fs::write(VALIDITY_UNIT_PATH, validity_unit(checkout))?;

    // Sensor firmware (downloads the vendor driver and extracts the blob).
    let fw = checkout.join("bin/validity-sensors-firmware");
    if fw.is_file() {
        println!("Fetching sensor firmware");
        let mut c = cmd("python3")?;
        c.arg(&fw).env("PYTHONPATH", checkout);
        if status(c)? != 0 {
            println!("warning: firmware download failed; the service may still work if firmware is already present");
        }
    }

    for args in [
        vec!["daemon-reload"],
        vec!["enable", "--now", "python3-validity.service"],
        vec!["restart", "open-fprintd.service"],
    ] {
        let mut c = cmd("systemctl")?;
        c.args(&args);
        let code = status(c)?;
        if code != 0 {
            return Ok(code);
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlists() {
        assert!(Action::AptInstall {
            packages: vec!["fprintd".into()]
        }
        .validate()
        .is_ok());
        assert!(Action::AptInstall {
            packages: vec!["fprintd; rm -rf /".into()]
        }
        .validate()
        .is_err());
        assert!(Action::AptInstall { packages: vec![] }.validate().is_err());
        assert!(Action::AptRemove {
            packages: vec!["howdy".into()]
        }
        .validate()
        .is_err());
        assert!(Action::AddPpa {
            ppa: "ppa:evil/stuff".into()
        }
        .validate()
        .is_err());
        assert!(Action::Systemctl {
            verb: "restart".into(),
            unit: "open-fprintd.service".into()
        }
        .validate()
        .is_ok());
        assert!(Action::Systemctl {
            verb: "restart".into(),
            unit: "sshd.service".into()
        }
        .validate()
        .is_err());
        assert!(Action::Systemctl {
            verb: "kill".into(),
            unit: "fprintd.service".into()
        }
        .validate()
        .is_err());
        assert!(Action::PamAuthUpdate {
            profile: "unix".into(),
            enable: false,
            force: false
        }
        .validate()
        .is_err());
    }

    #[test]
    fn howdy_settings() {
        assert!(check_howdy_setting("device_path", "/dev/video2").is_ok());
        assert!(check_howdy_setting(
            "device_path",
            "/dev/v4l/by-id/usb-HP_IR_Camera-video-index0"
        )
        .is_ok());
        assert!(check_howdy_setting("device_path", "/dev/video2; reboot").is_err());
        assert!(check_howdy_setting("device_path", "/etc/shadow").is_err());
        assert!(check_howdy_setting("device_path", "/dev/video").is_err());
        assert!(check_howdy_setting("certainty", "3.5").is_ok());
        assert!(check_howdy_setting("certainty", "99").is_err());
        assert!(check_howdy_setting("recording_plugin", "x").is_err());
    }

    #[test]
    fn labels_and_users() {
        assert!(check_label("Glasses").is_ok());
        assert!(check_label("").is_err());
        assert!(check_label("a,b").is_err());
        assert!(check_label("--help").is_err());
        assert!(check_label("x".repeat(25).as_str()).is_err());
        assert!(valid_user_name("kevin"));
        assert!(!valid_user_name("root;id"));
        assert!(!valid_user_name("-U"));
    }

    #[test]
    fn json_roundtrip() {
        let a = vec![
            Action::AptUpdate,
            Action::HowdyAdd {
                user: "u".into(),
                label: "x".into(),
            },
        ];
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("\"action\":\"howdy-add\""));
        assert_eq!(serde_json::from_str::<Vec<Action>>(&s).unwrap(), a);
        assert!(serde_json::from_str::<Vec<Action>>(r#"[{"action":"shell","cmd":"id"}]"#).is_err());
    }
}
