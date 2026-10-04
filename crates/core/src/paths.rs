//! Well-known file locations.

use std::env;
use std::path::PathBuf;

pub const APP_NAME: &str = "CoolerCast";

/// `%ProgramData%\CoolerCast`, shared by the service and the app.
pub fn data_dir() -> PathBuf {
    env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join(APP_NAME)
}

pub fn config_file() -> PathBuf {
    data_dir().join("config.toml")
}

pub fn log_file() -> PathBuf {
    data_dir().join("coolercast.log")
}

/// Locates a PawnIO module shipped with the app: `modules\` next to the executable, the
/// executable's own folder, or the repository copy in debug builds.
pub fn module_file(name: &str) -> Option<PathBuf> {
    let exe_dir = env::current_exe().ok()?.parent()?.to_path_buf();
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
