//! Unprivileged side of the helper: start `pkexec <self> --helper`, stream its
//! output back to the UI as events.

use crate::helper::Action;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub enum Event {
    Step {
        index: usize,
        total: usize,
        text: String,
    },
    Line(String),
    /// `Ok(())` on success, otherwise a short reason.
    Finished(Result<(), String>),
}

/// Strip ANSI colour codes some tools (Howdy) print.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

fn parse_line(line: &str) -> Event {
    if let Some(rest) = line.strip_prefix("##STEP ") {
        if let Some((frac, text)) = rest.split_once(' ') {
            if let Some((i, t)) = frac.split_once('/') {
                if let (Ok(index), Ok(total)) = (i.parse(), t.parse()) {
                    return Event::Step {
                        index,
                        total,
                        text: text.to_string(),
                    };
                }
            }
        }
    }
    Event::Line(strip_ansi(line))
}

pub fn run(actions: Vec<Action>) -> async_channel::Receiver<Event> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        let fail = |tx: &async_channel::Sender<Event>, msg: String| {
            let _ = tx.send_blocking(Event::Finished(Err(msg)));
        };
        let json = match serde_json::to_string(&actions) {
            Ok(j) => j,
            Err(e) => return fail(&tx, e.to_string()),
        };
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => return fail(&tx, format!("cannot locate own executable: {e}")),
        };
        let mut child = match Command::new("pkexec")
            .arg(exe)
            .arg("--helper")
            .arg(json)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return fail(&tx, format!("could not start pkexec: {e}")),
        };
        let stderr = child.stderr.take().unwrap();
        let tx_err = tx.clone();
        let err_thread = std::thread::spawn(move || {
            let mut last = String::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                last = strip_ansi(&line);
                let _ = tx_err.send_blocking(Event::Line(last.clone()));
            }
            last
        });
        let mut failed_step = None;
        for line in BufReader::new(child.stdout.take().unwrap())
            .lines()
            .map_while(Result::ok)
        {
            if line.starts_with("##DONE") {
                continue;
            }
            if let Some(rest) = line.strip_prefix("##FAILED ") {
                failed_step = rest.split_whitespace().next().map(str::to_string);
                continue;
            }
            let _ = tx.send_blocking(parse_line(&line));
        }
        let last_err = err_thread.join().unwrap_or_default();
        let result = match child.wait().map(|s| s.code()) {
            Ok(Some(0)) => Ok(()),
            // pkexec: 126 = dialog dismissed / not authorized, 127 = auth failed.
            Ok(Some(126)) | Ok(Some(127)) if failed_step.is_none() => {
                Err("Authorization was cancelled or denied".into())
            }
            Ok(code) => Err(match (failed_step, last_err.is_empty()) {
                (Some(step), _) => format!("Step {step} failed (exit code {})", code.unwrap_or(-1)),
                (None, false) => last_err,
                (None, true) => format!("Helper failed (exit code {})", code.unwrap_or(-1)),
            }),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send_blocking(Event::Finished(result));
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi() {
        assert_eq!(strip_ansi("\x1b[1;29mID  Date\x1b[0m\r"), "ID  Date");
    }

    #[test]
    fn step_lines() {
        match parse_line("##STEP 2/5 Install howdy") {
            Event::Step { index, total, text } => {
                assert_eq!((index, total, text.as_str()), (2, 5, "Install howdy"))
            }
            e => panic!("{e:?}"),
        }
        assert!(matches!(
            parse_line("Reading package lists..."),
            Event::Line(_)
        ));
    }
}
