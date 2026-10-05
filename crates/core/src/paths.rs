//! Well-known file locations.

use std::path::PathBuf;

pub const APP_NAME: &str = "CoolerCast";

/// `%ProgramData%\CoolerCast`, shared by the service and the app.
#[cfg(windows)]
pub fn data_dir() -> PathBuf {
    std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join(APP_NAME)
}

#[cfg(windows)]
pub fn config_file() -> PathBuf {
    data_dir().join("config.toml")
}

/// `/etc/coolercast/config.toml`, created by the systemd unit's `ConfigurationDirectory`.
#[cfg(target_os = "linux")]
pub fn config_file() -> PathBuf {
    PathBuf::from("/etc/coolercast/config.toml")
}

/// The service log. On Linux the service logs to the journal instead.
#[cfg(windows)]
pub fn log_file() -> PathBuf {
    data_dir().join("coolercast.log")
}

/// The control socket, in the systemd unit's `RuntimeDirectory`.
#[cfg(target_os = "linux")]
pub fn socket_file() -> PathBuf {
    PathBuf::from("/run/coolercast/coolercast.sock")
}

/// Locates a PawnIO module shipped with the app: `modules\` next to the executable, the
/// executable's own folder, or the repository copy in debug builds.
#[cfg(windows)]
pub fn module_file(name: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let mut candidates = vec![exe_dir.join("modules"), exe_dir];
    if cfg!(debug_assertions) {
        candidates.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/pawnio-modules"),
        );
    }
    candidates
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}
