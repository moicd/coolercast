//! Core of CoolerCast: device protocols, the HID transport, CPU sensors, configuration and the
//! IPC channel shared by the service, the CLI and the app. Runs on Windows and Linux (x86-64).

#[cfg(not(all(any(windows, target_os = "linux"), target_arch = "x86_64")))]
compile_error!("CoolerCast supports Windows and Linux on x86-64");

pub mod config;
pub mod device;
pub mod engine;
pub mod hid;
pub mod ipc;
pub mod log;
pub mod paths;
pub mod sensors;
#[cfg(windows)]
pub mod win;
