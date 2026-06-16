//! Minimal ANSI styling for human output (cli.md §2.2 `--no-color`, `NO_COLOR`).
//!
//! Color is a process-global, decided once in `main` and read by the renderers. It is
//! enabled only when the user did not opt out (`--no-color`/`NO_COLOR`) *and* stdout is a
//! terminal — so piping into a file or `jq` stays clean automatically.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

static COLOR: AtomicBool = AtomicBool::new(false);

/// Decide whether to colorize from the `--no-color` flag, the environment, and the tty.
pub fn auto(no_color_flag: bool) -> bool {
    !no_color_flag && std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

pub fn init(enabled: bool) {
    COLOR.store(enabled, Ordering::Relaxed);
}

fn on() -> bool {
    COLOR.load(Ordering::Relaxed)
}

fn wrap(code: &str, s: &str) -> String {
    if on() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn bold(s: &str) -> String {
    wrap("1", s)
}
pub fn dim(s: &str) -> String {
    wrap("2", s)
}
pub fn cyan(s: &str) -> String {
    wrap("36", s)
}
pub fn green(s: &str) -> String {
    wrap("32", s)
}
pub fn red(s: &str) -> String {
    wrap("31", s)
}
pub fn yellow(s: &str) -> String {
    wrap("33", s)
}
