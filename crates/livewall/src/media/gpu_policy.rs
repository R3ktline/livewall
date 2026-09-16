//! Hybrid GPU / decode-path selection helpers.

use std::fs;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

/// Pick a DRM render node for VA-API; prefer iGPU over NVIDIA.
#[derive(Debug, Clone, Default)]
pub struct GpuPolicy {
    pub preferred_drm_dev: Option<(u32, u32)>,
    pub render_node: Option<PathBuf>,
}

impl GpuPolicy {
    pub fn from_env() -> Self {
        let mut p = Self::default();
        if let Ok(node) = std::env::var("LIVEWALL_RENDER_NODE") {
            p.render_node = Some(PathBuf::from(node));
        }
        p
    }

    /// True when more than one GPU driver is present (e.g. Intel + NVIDIA).
    /// DMA-BUF from the iGPU will not show on a dGPU-wired HDMI.
    pub fn is_hybrid() -> bool {
        let mut drivers = std::collections::BTreeSet::new();
        let Ok(entries) = fs::read_dir("/dev/dri") else {
            return false;
        };
        for ent in entries.flatten() {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("renderD") {
                continue;
            }
            if let Some(d) = driver_name(&ent.path()) {
                drivers.insert(d);
            }
        }
        drivers.len() > 1
    }

    pub fn vaapi_device_path(&self) -> Option<String> {
        if let Some(node) = &self.render_node {
            return Some(node.display().to_string());
        }

        let mut nodes: Vec<(i32, PathBuf)> = Vec::new();
        let Ok(entries) = fs::read_dir("/dev/dri") else {
            return None;
        };
        for ent in entries.flatten() {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("renderD") {
                continue;
            }
            let path = ent.path();
            let score = score_render_node(&path);
            nodes.push((score, path));
        }
        nodes.sort_by_key(|(s, _)| *s);
        nodes.into_iter().next().map(|(_, p)| p.display().to_string())
    }

    pub fn log_selection(&self) {
        match self.vaapi_device_path() {
            Some(p) => info!(device = %p, "VA-API device preference"),
            None => warn!("no DRM render node found; ffmpeg will auto-select"),
        }
        if Self::is_hybrid() {
            info!("hybrid GPU detected — presenting SHM so HDMI/dGPU outputs work");
        }
        if let Some((maj, min)) = self.preferred_drm_dev {
            info!(maj, min, "compositor dmabuf main_device hint stored");
        }
    }
}

fn driver_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let sys = PathBuf::from("/sys/class/drm").join(name).join("device/driver");
    fs::read_link(&sys)
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
}

fn score_render_node(path: &Path) -> i32 {
    match driver_name(path).as_deref() {
        Some("i915" | "xe" | "amdgpu" | "radeon") => 0,
        Some("nvidia" | "nvidia_drm") => 50,
        Some("nouveau") => 40,
        _ => 20,
    }
}
