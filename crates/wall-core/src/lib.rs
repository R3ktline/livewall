//! Shared configuration and IPC types for livewall.

mod config;
mod ipc;
mod types;

pub use config::{load_config, save_config, Config, OutputConfig, PauseRules};
pub use ipc::{read_message, write_message, IpcError, Request, Response, StatusInfo, OutputStatus};
pub use types::{HwDecPreference, ScaleMode, WallpaperKind};

/// Default Unix socket name under `$XDG_RUNTIME_DIR`.
pub const SOCKET_NAME: &str = "walld.sock";

/// Resolve the IPC socket path.
pub fn socket_path() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return std::path::PathBuf::from(dir).join(SOCKET_NAME);
    }
    std::env::temp_dir().join(SOCKET_NAME)
}
