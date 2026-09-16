use std::io::{Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{HwDecPreference, ScaleMode, WallpaperKind};

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Client → daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Set {
        /// Output name, or `"ALL"`.
        output: String,
        path: PathBuf,
        mode: Option<ScaleMode>,
        /// Persist this assignment to the config file so it is restored on the
        /// next daemon start.
        #[serde(default)]
        save: bool,
    },
    Clear {
        output: String,
    },
    Pause,
    Resume,
    Status,
    Shutdown,
    /// Persist the currently displayed wallpapers to the config file.
    Save,
    SetHwdec {
        hwdec: HwDecPreference,
    },
}

/// Daemon → client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok { message: Option<String> },
    Error { message: String },
    Status(StatusInfo),
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusInfo {
    pub paused: bool,
    pub hwdec: String,
    pub decode_path: String,
    pub efficiency_warning: Option<String>,
    pub fps: f32,
    pub rss_bytes: u64,
    pub outputs: Vec<OutputStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputStatus {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
    pub path: Option<PathBuf>,
    pub kind: Option<WallpaperKind>,
    pub mode: ScaleMode,
    pub playing: bool,
}

const MAX_MSG: u32 = 16 * 1024 * 1024;

pub fn write_message<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), IpcError> {
    let bytes = serde_json::to_vec(msg)?;
    if bytes.len() as u32 > MAX_MSG {
        return Err(IpcError::Protocol("message too large".into()));
    }
    let len = (bytes.len() as u32).to_le_bytes();
    w.write_all(&len)?;
    w.write_all(&bytes)?;
    w.flush()?;
    Ok(())
}

pub fn read_message<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> Result<T, IpcError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_MSG {
        return Err(IpcError::Protocol(format!("message length {len} exceeds limit")));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(serde_json::from_slice(&buf)?)
}
