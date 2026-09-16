//! Decoded frames ready for Wayland presentation.

use std::os::fd::OwnedFd;

/// A single presentable frame.
#[derive(Debug)]
pub enum Frame {
    /// Packed XRGB8888 (little-endian) pixels in host memory.
    Shm {
        width: u32,
        height: u32,
        stride: u32,
        pixels: Vec<u8>,
    },
    /// Zero-copy DMA-BUF planes (DRM_PRIME).
    Dmabuf(DmabufFrame),
}

#[derive(Debug)]
pub struct DmabufFrame {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub modifier: u64,
    pub planes: Vec<DmabufPlane>,
}

#[derive(Debug)]
pub struct DmabufPlane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

impl Frame {
    pub fn width(&self) -> u32 {
        match self {
            Frame::Shm { width, .. } => *width,
            Frame::Dmabuf(d) => d.width,
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Frame::Shm { height, .. } => *height,
            Frame::Dmabuf(d) => d.height,
        }
    }

    /// Build an XRGB8888 SHM frame from tightly packed RGB8.
    pub fn from_rgb8(width: u32, height: u32, rgb: &[u8]) -> Self {
        let stride = width * 4;
        let mut pixels = vec![0u8; (stride * height) as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let si = (y * width as usize + x) * 3;
                let di = y * stride as usize + x * 4;
                pixels[di] = rgb[si + 2]; // B
                pixels[di + 1] = rgb[si + 1]; // G
                pixels[di + 2] = rgb[si]; // R
                pixels[di + 3] = 0xff;
            }
        }
        Frame::Shm {
            width,
            height,
            stride,
            pixels,
        }
    }

    pub fn from_rgba8(width: u32, height: u32, rgba: &[u8]) -> Self {
        let stride = width * 4;
        let mut pixels = vec![0u8; (stride * height) as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let si = (y * width as usize + x) * 4;
                let di = y * stride as usize + x * 4;
                pixels[di] = rgba[si + 2];
                pixels[di + 1] = rgba[si + 1];
                pixels[di + 2] = rgba[si];
                pixels[di + 3] = rgba[si + 3];
            }
        }
        Frame::Shm {
            width,
            height,
            stride,
            pixels,
        }
    }
}

/// Active decode backend reported in status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecodePath {
    #[default]
    None,
    Image,
    Soft,
    VaapiDmabuf,
    #[allow(dead_code)]
    VulkanVideo,
}

impl DecodePath {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Image => "image",
            Self::Soft => "soft",
            Self::VaapiDmabuf => "vaapi-dmabuf",
            Self::VulkanVideo => "vulkan-video",
        }
    }
}
