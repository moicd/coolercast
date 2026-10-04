//! Core of CoolerCast: device protocols, the Win32 HID transport, CPU sensors,
//! configuration and the IPC channel shared by the service and the tray app.

pub mod config;
pub mod device;
pub mod engine;
pub mod hid;
pub mod ipc;
pub mod log;
pub mod paths;
pub mod sensors;
pub mod win;
