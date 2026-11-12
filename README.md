# simple-auth-tool

A small native (GTK4 + libadwaita) app that sets up **fingerprint** and **face** login on Linux.

It **does not implement any authentication itself**. It finds your hardware,
works out which existing project drives it, installs that, and then uses the
project's own tools to enroll you and turn it on for login:

| Hardware | Backend it sets up |
|---|---|
| Fingerprint readers supported by libfprint | `fprintd` + `libpam-fprintd` |
| Synaptics/Validity sensors libfprint can't drive | `open-fprintd` + `python-validity` (PPA, or a source checkout of a fork) |
| IR camera (or a webcam, with a warning) | [Howdy](https://github.com/boltgolt/howdy) |
| Login, sudo, lock screen | `pam-auth-update` profiles `fprintd` / `howdy` |

## What it does

1. **Detects** USB fingerprint readers (sysfs) and V4L2 cameras, marking IR ones.
   Readers are matched against libfprint's own hwdb
   (`60-autosuspend-fingerprint-reader.hwdb`, the list of supported and known
   unsupported devices) and against [`data/devices.toml`](data/devices.toml) for
   sensors that need python-validity. It also checks installed packages, the
   systemd services, what fprintd sees over D-Bus, and the PAM state.
2. **Sets up** whatever is missing in one administrator step (via `pkexec`), for
   example adding `ppa:uunicorn/open-fprintd`, removing a conflicting stock
   `fprintd`, installing packages, installing the python-validity service from a
   checkout, or pointing Howdy at the IR camera.
3. **Enrolls** a profile (any regular user account):
   - fingerprints over fprintd's D-Bus API: pick a finger, touch the sensor, watch progress, test, delete;
   - faces with `howdy add/remove/clear`, plus a recognition test via Howdy's `compare.py`.
4. **Turns it on** for login/sudo with `pam-auth-update --enable/--disable`. It
   never edits `/etc/pam.d` itself. If those files were edited by hand, it asks
   before passing `--force`.

## Build

Needs Rust ≥ 1.75 and the GTK4/libadwaita development files:

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev
cargo build --release
```

## Run

```bash
cargo run --release              # the app
cargo run -- --detect            # JSON report of what was detected, no UI
```

`SIMPLE_AUTH_TOOL_OPEN=face` (or `usb:vvvv:pppp`) opens that device's page at startup.

## Install

Download the latest `.deb` and install it with apt, which also pulls in the dependencies.
Ubuntu 24.04+, Linux Mint 22+, Pop!_OS 24.04 and Debian 13 work; it needs
libadwaita ≥ 1.5.

```bash
wget https://github.com/EarvinInk/simple_linux_auth_tool/releases/latest/download/simple-auth-tool_amd64.deb
sudo apt install ./simple-auth-tool_amd64.deb
```

It then appears in the menu as **Fingerprint & Face Login**. To remove it:
`sudo apt remove simple-auth-tool`.

The package installs the binary, a `.desktop` entry and a polkit policy, so the
password prompt says what it's for.

### Build the package yourself

```bash
sudo apt install libgtk-4-dev libadwaita-1-dev
cargo install cargo-deb
cargo deb                                   # -> target/debian/simple-auth-tool_<ver>-1_amd64.deb
sudo apt install ./target/debian/simple-auth-tool_*.deb
```

### Making a release

CI ([.github/workflows/release.yml](.github/workflows/release.yml)) lints, tests and
builds the `.deb` on every push. To publish a release, bump `version` in
`Cargo.toml`, commit, then push a matching tag:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

The workflow attaches the versioned `.deb`, a fixed-name `simple-auth-tool_amd64.deb`
(what the `latest/download` link above points to) and `SHA256SUMS` to the release.
To offer it on your own website, link to that `latest/download` URL.

## Privilege model

The UI never runs as root. Administrator work runs as
`pkexec simple-auth-tool --helper '<json>'`, where the JSON is a list of typed
actions (`apt-install`, `add-ppa`, `systemctl`, `pam-auth-update`, `howdy-add`, …).
The helper checks every action against a fixed allowlist before running anything:

- known package names and PPAs only;
- known systemd units and verbs only;
- the `fprintd`/`howdy` PAM profiles only;
- Howdy keys `device_path` (`/dev/videoN` or `/dev/v4l/by-id/...`), `certainty`, `timeout`, `dark_threshold`;
- only existing regular user accounts;
- a python-validity checkout must contain `dbus_service/dbus-service` and must not be world-writable.

It runs commands directly (no shell) with a clean environment, and stops at the
first failure. Fingerprint enrollment needs no root: fprintd's own polkit rules apply.

## Adding sensors

Describe a sensor in `~/.config/simple-auth-tool/devices.toml` (same format as
[`data/devices.toml`](data/devices.toml)), for example a reader that only your
python-validity fork supports:

```toml
[[device]]
usb = "06cb:00b7"
name = "Synaptics FM-115 (HP G6 series)"
backend = "python-validity"
validity = "local"
```

The checkout path you choose in the app is saved in `~/.config/simple-auth-tool/config.toml`.

## Layout

```
src/detect/      USB, cameras, libfprint hwdb, packages/services/users
src/backends/    per-device evaluation (mod.rs), fprintd D-Bus, Howdy, PAM
src/helper.rs    root-side allowlisted actions
src/privileged.rs  pkexec runner with streamed output
src/ui/          overview, device page, fingerprint and face enrollment, log dialog
```
