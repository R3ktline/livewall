//! Video decode via native FFmpeg bridge (`native/wall_decode.c`).

use std::ffi::{CStr, CString};
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::Path;
use std::ptr;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use wall_core::HwDecPreference;

use super::frame::{DecodePath, DmabufFrame, DmabufPlane, Frame};
use super::GpuPolicy;

#[repr(C)]
struct WallRgbFrame {
    data: *mut u8,
    width: i32,
    height: i32,
    stride: i32,
}

#[repr(C)]
struct WallDmabufPlaneC {
    fd: i32,
    offset: u32,
    stride: u32,
}

#[repr(C)]
struct WallDmabufFrameC {
    width: u32,
    height: u32,
    format: u32,
    modifier: u64,
    nb_planes: i32,
    planes: [WallDmabufPlaneC; 4],
}

enum WallDecoder {}

extern "C" {
    fn wall_decoder_open(
        path: *const libc::c_char,
        vaapi_device: *const libc::c_char,
        want_hw: i32,
        efficiency: i32,
        force_rgb: i32,
    ) -> *mut WallDecoder;
    fn wall_decoder_free(d: *mut WallDecoder);
    fn wall_decoder_is_hw(d: *const WallDecoder) -> i32;
    fn wall_decoder_fps(d: *const WallDecoder) -> f32;
    fn wall_decoder_warning(d: *const WallDecoder) -> *const libc::c_char;
    fn wall_decoder_set_target_size(d: *mut WallDecoder, w: i32, h: i32);
    fn wall_decoder_width(d: *const WallDecoder) -> i32;
    fn wall_decoder_height(d: *const WallDecoder) -> i32;
    fn wall_decoder_seek_start(d: *mut WallDecoder) -> i32;
    fn wall_decoder_next(
        d: *mut WallDecoder,
        rgb: *mut WallRgbFrame,
        dma: *mut WallDmabufFrameC,
    ) -> i32;
    fn wall_rgb_free(f: *mut WallRgbFrame);
    fn wall_dmabuf_close(f: *mut WallDmabufFrameC);
}

pub struct VideoPlayerConfig {
    pub hwdec: HwDecPreference,
    pub efficiency_mode: bool,
    pub max_fps: u32,
    /// Scale decoded frames down to this size (0 = native).
    pub target_w: u32,
    pub target_h: u32,
}

pub struct VideoPlayer {
    dec: *mut WallDecoder,
    decode_path: DecodePath,
    warning: Option<String>,
    fps: f32,
    frame_period: Duration,
    last_present: Option<Instant>,
    max_fps: u32,
}

unsafe impl Send for VideoPlayer {}

impl VideoPlayer {
    pub fn open(path: &Path, cfg: VideoPlayerConfig) -> Result<Self> {
        let path_c = CString::new(path.to_string_lossy().as_bytes())?;
        let policy = GpuPolicy::from_env();
        policy.log_selection();
        let device = policy.vaapi_device_path().and_then(|s| CString::new(s).ok());

        let (want_hw, force_path) = match cfg.hwdec {
            HwDecPreference::Soft => (0, Some(DecodePath::Soft)),
            HwDecPreference::Vulkan => {
                // Vulkan Video not wired yet — use VA-API zero-copy path.
                (1, None)
            }
            HwDecPreference::Vaapi | HwDecPreference::Auto => (1, None),
        };

        if cfg.hwdec == HwDecPreference::Soft && cfg.efficiency_mode {
            bail!("efficiency_mode forbids software decode");
        }

        let force_rgb = GpuPolicy::is_hybrid() || std::env::var_os("WALLD_FORCE_SHM").is_some();

        let dec = unsafe {
            wall_decoder_open(
                path_c.as_ptr(),
                device
                    .as_ref()
                    .map(|c| c.as_ptr())
                    .unwrap_or(ptr::null()),
                want_hw,
                if cfg.efficiency_mode { 1 } else { 0 },
                if force_rgb { 1 } else { 0 },
            )
        };
        if dec.is_null() {
            bail!("failed to open video decoder for {}", path.display());
        }

        if cfg.target_w > 0 && cfg.target_h > 0 {
            let src_w = unsafe { wall_decoder_width(dec) } as u32;
            let src_h = unsafe { wall_decoder_height(dec) } as u32;
            // Only downscale — never upscale past source.
            let tw = cfg.target_w.min(src_w.max(1));
            let th = cfg.target_h.min(src_h.max(1));
            if tw < src_w || th < src_h {
                unsafe { wall_decoder_set_target_size(dec, tw as i32, th as i32) };
                tracing::info!(tw, th, src_w, src_h, "decode target size set");
            }
        }

        let is_hw = unsafe { wall_decoder_is_hw(dec) } != 0;
        let fps = unsafe { wall_decoder_fps(dec) }.max(1.0);
        let warn_ptr = unsafe { wall_decoder_warning(dec) };
        let mut warning = if warn_ptr.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(warn_ptr) }.to_string_lossy().into_owned())
        };

        let decode_path = if let Some(p) = force_path {
            p
        } else if is_hw && force_rgb {
            DecodePath::VaapiDmabuf // hw decode; SHM present (hybrid)
        } else if is_hw {
            if cfg.hwdec == HwDecPreference::Vulkan {
                warning = Some(
                    warning.unwrap_or_else(|| {
                        "Vulkan Video decode not yet wired; using VA-API DMA-BUF".into()
                    }),
                );
            }
            DecodePath::VaapiDmabuf
        } else {
            if cfg.efficiency_mode {
                unsafe { wall_decoder_free(dec) };
                bail!("efficiency_mode requires VA-API; hardware decode unavailable");
            }
            DecodePath::Soft
        };

        // Clarify status string for hybrid SHM present
        if is_hw && force_rgb {
            warning = Some(
                warning.unwrap_or_else(|| {
                    "hybrid GPU: VA-API decode, SHM present (works on HDMI/dGPU)".into()
                }),
            );
        } else if decode_path == DecodePath::Soft && warning.is_none() {
            warning = Some("using software decode (higher CPU/RAM)".into());
        }

        Ok(Self {
            dec,
            decode_path,
            warning,
            fps,
            frame_period: Duration::from_secs_f32(1.0 / fps),
            last_present: None,
            max_fps: cfg.max_fps,
        })
    }

    pub fn decode_path(&self) -> DecodePath {
        self.decode_path
    }

    pub fn warning(&self) -> Option<&String> {
        self.warning.as_ref()
    }

    pub fn fps(&self) -> f32 {
        self.fps
    }

    pub fn seek_start(&mut self) -> Result<()> {
        let ret = unsafe { wall_decoder_seek_start(self.dec) };
        if ret < 0 {
            bail!("seek failed ({ret})");
        }
        self.last_present = None;
        Ok(())
    }

    pub fn next_frame(&mut self) -> Result<Option<(Frame, Duration)>> {
        let mut rgb = WallRgbFrame {
            data: ptr::null_mut(),
            width: 0,
            height: 0,
            stride: 0,
        };
        let mut dma = WallDmabufFrameC {
            width: 0,
            height: 0,
            format: 0,
            modifier: 0,
            nb_planes: 0,
            planes: [
                WallDmabufPlaneC {
                    fd: -1,
                    offset: 0,
                    stride: 0,
                },
                WallDmabufPlaneC {
                    fd: -1,
                    offset: 0,
                    stride: 0,
                },
                WallDmabufPlaneC {
                    fd: -1,
                    offset: 0,
                    stride: 0,
                },
                WallDmabufPlaneC {
                    fd: -1,
                    offset: 0,
                    stride: 0,
                },
            ],
        };

        let code = unsafe { wall_decoder_next(self.dec, &mut rgb, &mut dma) };
        match code {
            0 => Ok(None),
            -1 => bail!("decode error"),
            1 => {
                if rgb.data.is_null() {
                    bail!("null rgb frame");
                }
                let w = rgb.width as u32;
                let h = rgb.height as u32;
                let stride = rgb.stride as u32;
                let len = (rgb.stride * rgb.height) as usize;
                let slice = unsafe { std::slice::from_raw_parts(rgb.data, len) };
                // Decoder emits BGR0 / XRGB8888 already — take bytes, free C buffer.
                let mut pixels = Vec::with_capacity(len);
                pixels.extend_from_slice(slice);
                unsafe { wall_rgb_free(&mut rgb) };
                let frame = Frame::from_xrgb8(w, h, stride, pixels);
                Ok(Some((frame, self.pace())))
            }
            2 => {
                let n = dma.nb_planes as usize;
                let mut planes = Vec::new();
                for i in 0..n {
                    let fd_raw = dma.planes[i].fd;
                    let offset = dma.planes[i].offset;
                    let stride = dma.planes[i].stride;
                    if fd_raw < 0 {
                        continue;
                    }
                    dma.planes[i].fd = -1;
                    let fd = unsafe { OwnedFd::from_raw_fd(fd_raw) };
                    planes.push(DmabufPlane {
                        fd,
                        offset,
                        stride,
                    });
                }
                let frame = Frame::Dmabuf(DmabufFrame {
                    width: dma.width,
                    height: dma.height,
                    format: dma.format,
                    modifier: dma.modifier,
                    planes,
                });
                unsafe { wall_dmabuf_close(&mut dma) };
                self.decode_path = DecodePath::VaapiDmabuf;
                Ok(Some((frame, self.pace())))
            }
            other => bail!("unexpected decoder code {other}"),
        }
    }

    fn pace(&mut self) -> Duration {
        let mut period = self.frame_period;
        if self.max_fps > 0 {
            let cap = Duration::from_secs_f32(1.0 / self.max_fps as f32);
            if cap > period {
                period = cap;
            }
        }
        let now = Instant::now();
        let delay = if let Some(last) = self.last_present {
            period.saturating_sub(now.saturating_duration_since(last))
        } else {
            Duration::ZERO
        };
        self.last_present = Some(now + delay);
        delay
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        if !self.dec.is_null() {
            unsafe { wall_decoder_free(self.dec) };
            self.dec = ptr::null_mut();
        }
    }
}
