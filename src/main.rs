mod app;
mod backends;
mod config;
mod detect;
mod helper;
mod privileged;
mod ui;

use std::process::ExitCode;

const USAGE: &str = "\
simple-auth-tool — set up fingerprint and face login on Linux

USAGE:
    simple-auth-tool             open the app
    simple-auth-tool --detect    print what was detected as JSON
    simple-auth-tool --help      show this help

The app never implements authentication itself; it drives fprintd,
open-fprintd/python-validity, Howdy and pam-auth-update.";

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--helper") => helper::main(&args[2..]),
        Some("--detect") => {
            let report = detect::run(&config::Settings::load());
            match serde_json::to_string_pretty(&report) {
                Ok(s) => {
                    println!("{s}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--help") | Some("-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) if other.starts_with('-') => {
            eprintln!("unknown option {other}\n\n{USAGE}");
            ExitCode::from(2)
        }
        _ => app::run(),
    }
}
