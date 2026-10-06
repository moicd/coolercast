//! coolercast-app: tray icon showing the CPU temperature and a settings window for the
//! CoolerCast service.
//!
//! Started without arguments it opens the settings window; `--tray` (used at sign-in) starts
//! with the icon only. A second start brings the running instance's window to the front.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod anim;
#[cfg(windows)]
mod autostart;
#[cfg(windows)]
mod gfx;
#[cfg(windows)]
mod i18n;
#[cfg(windows)]
mod icon;
#[cfg(windows)]
mod preview;
#[cfg(windows)]
mod theme;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod update;
#[cfg(windows)]
mod window;

#[cfg(windows)]
fn main() {
    tray::main();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("coolercast-app is only available on Windows; on Linux, use the coolercast command.");
    std::process::exit(1);
}
