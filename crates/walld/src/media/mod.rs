mod frame;
mod gpu_policy;
mod image_player;
mod video_player;

pub use frame::{DecodePath, DmabufFrame, Frame};
pub use gpu_policy::GpuPolicy;
pub use image_player::ImagePlayer;
pub use video_player::{VideoPlayer, VideoPlayerConfig};

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use wall_core::{HwDecPreference, WallpaperKind};

/// Shared handle the Wayland thread pulls frames from.
#[derive(Clone, Default)]
pub struct MediaSlot {
    inner: Arc<Mutex<MediaSlotInner>>,
}

impl std::fmt::Debug for MediaSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MediaSlot")
    }
}

#[derive(Default)]
struct MediaSlotInner {
    path: Option<PathBuf>,
    kind: Option<WallpaperKind>,
    gen: u64,
    latest: Option<std::sync::Arc<Frame>>,
    decode_path: DecodePath,
    warning: Option<String>,
    fps: f32,
    playing: bool,
}

impl MediaSlot {
    pub fn set_frame(&self, frame: Frame, path: DecodePath, fps: f32) {
        let mut g = self.inner.lock().unwrap();
        g.gen = g.gen.wrapping_add(1);
        g.latest = Some(std::sync::Arc::new(frame));
        g.decode_path = path;
        g.fps = fps;
        g.playing = true;
    }

    /// Return a new frame for this output if `last_gen` is behind.
    pub fn frame_if_newer(&self, last_gen: &mut u64) -> Option<std::sync::Arc<Frame>> {
        let g = self.inner.lock().unwrap();
        if g.gen != *last_gen {
            *last_gen = g.gen;
            g.latest.clone()
        } else {
            None
        }
    }

    pub fn peek_meta(&self) -> (Option<PathBuf>, Option<WallpaperKind>, DecodePath, Option<String>, f32, bool) {
        let g = self.inner.lock().unwrap();
        (
            g.path.clone(),
            g.kind,
            g.decode_path,
            g.warning.clone(),
            g.fps,
            g.playing,
        )
    }

    pub fn set_source(&self, path: PathBuf, kind: WallpaperKind) {
        let mut g = self.inner.lock().unwrap();
        g.path = Some(path);
        g.kind = Some(kind);
    }

    pub fn set_warning(&self, w: Option<String>) {
        self.inner.lock().unwrap().warning = w;
    }

    pub fn set_playing(&self, playing: bool) {
        self.inner.lock().unwrap().playing = playing;
    }

    pub fn set_decode_path(&self, p: DecodePath) {
        self.inner.lock().unwrap().decode_path = p;
    }
}

pub struct PlayerHandle {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl PlayerHandle {
    #[allow(dead_code)]
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for PlayerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Spawn the appropriate player for `path` into `slot`.
pub fn spawn_player(
    path: &Path,
    slot: MediaSlot,
    hwdec: HwDecPreference,
    efficiency_mode: bool,
    max_fps: u32,
    target_w: u32,
    target_h: u32,
    paused: Arc<AtomicBool>,
) -> Result<PlayerHandle> {
    let kind = WallpaperKind::from_path(path);
    slot.set_source(path.to_path_buf(), kind);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_c = stop.clone();
    let path = path.to_path_buf();

    let join = std::thread::Builder::new()
        .name("walld-media".into())
        .spawn(move || {
            let res = match kind {
                WallpaperKind::Image | WallpaperKind::AnimatedImage => {
                    run_image(&path, slot.clone(), kind, stop_c, paused)
                }
                WallpaperKind::Video => run_video(
                    &path,
                    slot.clone(),
                    hwdec,
                    efficiency_mode,
                    max_fps,
                    target_w,
                    target_h,
                    stop_c,
                    paused,
                ),
            };
            if let Err(e) = res {
                tracing::error!(error = %e, "media player exited with error");
                slot.set_warning(Some(e.to_string()));
                slot.set_playing(false);
            }
        })?;

    Ok(PlayerHandle {
        stop,
        join: Some(join),
    })
}

fn run_image(
    path: &Path,
    slot: MediaSlot,
    kind: WallpaperKind,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) -> Result<()> {
    let mut player = ImagePlayer::open(path)?;
    slot.set_decode_path(DecodePath::Image);
    slot.set_warning(None);

    if kind == WallpaperKind::Image {
        let frame = player.next_frame()?.context("empty image")?;
        slot.set_frame(frame, DecodePath::Image, 0.0);
        // Keep thread alive until stopped so Drop semantics stay simple.
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
        return Ok(());
    }

    // Animated GIF / WebP
    while !stop.load(Ordering::Relaxed) {
        if paused.load(Ordering::Relaxed) {
            slot.set_playing(false);
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        match player.next_frame()? {
            Some(frame) => {
                let fps = player.last_delay().as_secs_f32().recip().clamp(1.0, 60.0);
                slot.set_frame(frame, DecodePath::Image, fps);
                std::thread::sleep(player.last_delay());
            }
            None => {
                player.reset()?;
            }
        }
    }
    Ok(())
}

fn run_video(
    path: &Path,
    slot: MediaSlot,
    hwdec: HwDecPreference,
    efficiency_mode: bool,
    max_fps: u32,
    target_w: u32,
    target_h: u32,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) -> Result<()> {
    let cfg = VideoPlayerConfig {
        hwdec,
        efficiency_mode,
        max_fps,
        target_w,
        target_h,
    };
    let mut player = VideoPlayer::open(path, cfg)?;
    slot.set_decode_path(player.decode_path());
    slot.set_warning(player.warning().cloned());

    while !stop.load(Ordering::Relaxed) {
        if paused.load(Ordering::Relaxed) {
            slot.set_playing(false);
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        match player.next_frame()? {
            Some((frame, pts_delay)) => {
                let fps = player.fps();
                slot.set_frame(frame, player.decode_path(), fps);
                if pts_delay > Duration::ZERO {
                    std::thread::sleep(pts_delay);
                }
            }
            None => {
                player.seek_start()?;
            }
        }
    }
    Ok(())
}

pub fn validate_path(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("path does not exist: {}", path.display());
    }
    if !path.is_file() {
        bail!("path is not a file: {}", path.display());
    }
    Ok(())
}
