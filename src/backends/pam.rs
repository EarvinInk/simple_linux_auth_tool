//! Read-only view of PAM state. Changes always go through pam-auth-update
//! (see helper::Action::PamAuthUpdate); /etc/pam.d is never edited directly.

use serde::Serialize;

pub const COMMON_AUTH: &str = "/etc/pam.d/common-auth";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LineState {
    Enabled,
    /// The module line is present but commented out, i.e. edited by hand.
    Commented,
    Absent,
}

#[derive(Debug, Clone, Serialize)]
pub struct PamMethod {
    /// pam-auth-update profile name ("fprintd" or "howdy").
    pub profile: &'static str,
    /// /usr/share/pam-configs/<profile> exists.
    pub profile_installed: bool,
    pub common_auth: LineState,
}

impl PamMethod {
    pub fn enabled(&self) -> bool {
        self.common_auth == LineState::Enabled
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PamState {
    pub fprintd: PamMethod,
    pub howdy: PamMethod,
}

pub fn line_state(common_auth: &str, module: &str) -> LineState {
    let mut state = LineState::Absent;
    for line in common_auth.lines() {
        let t = line.trim();
        if !t.contains(module) {
            continue;
        }
        if t.starts_with('#') {
            if state == LineState::Absent {
                state = LineState::Commented;
            }
        } else {
            return LineState::Enabled;
        }
    }
    state
}

pub fn read() -> PamState {
    let text = std::fs::read_to_string(COMMON_AUTH).unwrap_or_default();
    let method = |profile: &'static str, module: &str| PamMethod {
        profile,
        profile_installed: std::path::Path::new("/usr/share/pam-configs")
            .join(profile)
            .is_file(),
        common_auth: line_state(&text, module),
    };
    PamState {
        fprintd: method("fprintd", "pam_fprintd.so"),
        howdy: method("howdy", "pam_howdy.so"),
    }
}

/// pam-auth-update refuses to touch files that were edited by hand.
pub fn output_reports_local_modifications(output: &str) -> bool {
    output.contains("Local modifications")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# here are the per-package modules (the \"Primary\" block)
#auth\t[success=5 default=ignore]        /usr/lib/security/howdy/pam_howdy.so
auth\t[success=4 default=ignore]\tpam_fprintd.so max_tries=1 timeout=10 # debug
auth\t[success=3 default=ignore]\tpam_unix.so nullok try_first_pass
";

    #[test]
    fn detects_states() {
        assert_eq!(line_state(SAMPLE, "pam_fprintd.so"), LineState::Enabled);
        assert_eq!(line_state(SAMPLE, "pam_howdy.so"), LineState::Commented);
        assert_eq!(line_state(SAMPLE, "pam_u2f.so"), LineState::Absent);
    }

    #[test]
    fn trailing_comment_does_not_disable() {
        // "# debug" at the end of the fprintd line must not count as commented out.
        assert_eq!(
            line_state("auth x pam_fprintd.so # debug\n", "pam_fprintd.so"),
            LineState::Enabled
        );
    }
}
