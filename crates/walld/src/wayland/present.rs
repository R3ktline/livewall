//! Present SHM / DMA-BUF frames onto a wl_surface.

use std::os::fd::{AsFd, OwnedFd};

use anyhow::{bail, Result};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{Format as ShmFormat, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::Flags;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;

use crate::media::{DmabufFrame, Frame};

use super::state::AppData;

pub struct ShmPool {
    _fd: OwnedFd,
    pool: WlShmPool,
    pub size: usize,
    mmap: memmap2::MmapMut,
}

impl ShmPool {
    pub fn create(shm: &WlShm, qh: &QueueHandle<AppData>, size: usize) -> Result<Self> {
        let fd = rustix::fs::memfd_create(
            "walld-shm",
            rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
        )?;
        rustix::fs::ftruncate(&fd, size as u64)?;
        let mmap = unsafe { memmap2::MmapMut::map_mut(&fd)? };
        if mmap.len() < size {
            bail!("mmap shorter than requested");
        }
        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        Ok(Self {
            _fd: fd,
            pool,
            size,
            mmap,
        })
    }

    pub fn write_xrgb(&mut self, pixels: &[u8]) -> Result<()> {
        if pixels.len() > self.size {
            bail!("frame larger than shm pool");
        }
        self.mmap[..pixels.len()].copy_from_slice(pixels);
        Ok(())
    }

    pub fn create_buffer(
        &self,
        qh: &QueueHandle<AppData>,
        width: i32,
        height: i32,
        stride: i32,
    ) -> WlBuffer {
        self.pool
            .create_buffer(0, width, height, stride, ShmFormat::Xrgb8888, qh, ())
    }
}

pub fn present_frame(
    conn: &Connection,
    surface: &WlSurface,
    viewport: Option<&WpViewport>,
    shm: &WlShm,
    dmabuf: Option<&ZwpLinuxDmabufV1>,
    qh: &QueueHandle<AppData>,
    frame: &Frame,
    out_w: u32,
    out_h: u32,
    scale_mode: wall_core::ScaleMode,
    pools: &mut Vec<ShmPool>,
) -> Result<()> {
    let (src_w, src_h) = (frame.width(), frame.height());
    apply_viewport(viewport, src_w, src_h, out_w, out_h, scale_mode);

    match frame {
        Frame::Shm {
            width,
            height,
            stride,
            pixels,
        } => {
            let need = pixels.len();
            if pools.is_empty() || pools[0].size < need {
                pools.clear();
                pools.push(ShmPool::create(shm, qh, need.next_multiple_of(4096))?);
            }
            pools[0].write_xrgb(pixels)?;
            let buf = pools[0].create_buffer(qh, *width as i32, *height as i32, *stride as i32);
            surface.attach(Some(&buf), 0, 0);
            surface.damage_buffer(0, 0, *width as i32, *height as i32);
            surface.commit();
            buf.destroy();
            let _ = conn.flush();
        }
        Frame::Dmabuf(dma) => {
            let Some(dmabuf) = dmabuf else {
                bail!("DMA-BUF frame but linux-dmabuf protocol unavailable");
            };
            present_dmabuf(conn, surface, dmabuf, qh, dma)?;
        }
    }
    Ok(())
}

pub fn apply_viewport(
    viewport: Option<&WpViewport>,
    src_w: u32,
    src_h: u32,
    out_w: u32,
    out_h: u32,
    mode: wall_core::ScaleMode,
) {
    let Some(vp) = viewport else { return };
    vp.set_destination(out_w as i32, out_h as i32);

    let (crop_x, crop_y, crop_w, crop_h) = match mode {
        wall_core::ScaleMode::Stretch => (0.0, 0.0, src_w as f64, src_h as f64),
        wall_core::ScaleMode::Center => {
            let w = (src_w.min(out_w)) as f64;
            let h = (src_h.min(out_h)) as f64;
            let x = ((src_w as f64) - w) / 2.0;
            let y = ((src_h as f64) - h) / 2.0;
            (x, y, w, h)
        }
        wall_core::ScaleMode::Fit => (0.0, 0.0, src_w as f64, src_h as f64),
        wall_core::ScaleMode::Fill => {
            let src_a = src_w as f64 / src_h as f64;
            let out_a = out_w as f64 / out_h as f64;
            if src_a > out_a {
                let w = src_h as f64 * out_a;
                let x = (src_w as f64 - w) / 2.0;
                (x, 0.0, w, src_h as f64)
            } else {
                let h = src_w as f64 / out_a;
                let y = (src_h as f64 - h) / 2.0;
                (0.0, y, src_w as f64, h)
            }
        }
    };

    vp.set_source(crop_x, crop_y, crop_w, crop_h);
}

fn present_dmabuf(
    conn: &Connection,
    surface: &WlSurface,
    dmabuf: &ZwpLinuxDmabufV1,
    qh: &QueueHandle<AppData>,
    frame: &DmabufFrame,
) -> Result<()> {
    let params = dmabuf.create_params(qh, ());
    for (i, plane) in frame.planes.iter().enumerate() {
        params.add(
            plane.fd.as_fd(),
            i as u32,
            plane.offset,
            plane.stride,
            (frame.modifier >> 32) as u32,
            (frame.modifier & 0xffff_ffff) as u32,
        );
    }
    let buf = params.create_immed(
        frame.width as i32,
        frame.height as i32,
        frame.format,
        Flags::empty(),
        qh,
        (),
    );
    params.destroy();
    surface.attach(Some(&buf), 0, 0);
    surface.damage_buffer(0, 0, frame.width as i32, frame.height as i32);
    surface.commit();
    buf.destroy();
    let _ = conn.flush();
    Ok(())
}
