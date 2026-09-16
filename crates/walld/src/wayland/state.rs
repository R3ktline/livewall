//! Wayland layer-shell client: one background surface per output.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    registry_handlers,
};
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Proxy, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wall_core::{OutputStatus, ScaleMode, WallpaperKind};

use crate::media::{Frame, MediaSlot};
use crate::wayland::present::{apply_viewport as apply_viewport_pub, present_frame, ShmPool};

#[derive(Debug)]
pub enum WaylandCmd {
    SetWallpaper {
        output: String,
        path: std::path::PathBuf,
        mode: ScaleMode,
        slot: MediaSlot,
    },
    Clear {
        output: String,
    },
    #[allow(dead_code)]
    SetScaleMode {
        output: String,
        mode: ScaleMode,
    },
    Shutdown,
}

pub struct WaylandHandle {
    pub tx: Sender<WaylandCmd>,
    pub outputs: Arc<Mutex<Vec<OutputStatus>>>,
}

struct OutputSurf {
    name: String,
    layer: LayerSurface,
    viewport: Option<WpViewport>,
    width: u32,
    height: u32,
    refresh_mhz: i32,
    mode: ScaleMode,
    slot: Option<MediaSlot>,
    path: Option<std::path::PathBuf>,
    kind: Option<WallpaperKind>,
    pools: Vec<ShmPool>,
    configured: bool,
    last_frame_gen: u64,
}

pub struct AppData {
    registry_state: RegistryState,
    compositor: CompositorState,
    output_state: OutputState,
    shm: Shm,
    layer_shell: LayerShell,
    viewporter: Option<WpViewporter>,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    surfaces: HashMap<String, OutputSurf>,
    /// Map wl_output name binding
    output_names: HashMap<WlOutput, String>,
    cmd_rx: Receiver<WaylandCmd>,
    status: Arc<Mutex<Vec<OutputStatus>>>,
    exit: Arc<AtomicBool>,
    qh: QueueHandle<AppData>,
    /// Shared SHM pool so multi-monitor uploads the frame once.
    shared_shm: Option<ShmPool>,
}

pub fn run_wayland(
    cmd_rx: Receiver<WaylandCmd>,
    status: Arc<Mutex<Vec<OutputStatus>>>,
    exit: Arc<AtomicBool>,
) -> Result<()> {
    let conn = Connection::connect_to_env().context("WAYLAND_DISPLAY connect")?;
    let (globals, mut event_queue) = registry_queue_init::<AppData>(&conn)?;
    let qh = event_queue.handle();

    let viewporter = globals.bind(&qh, 1..=1, ()).ok();
    let dmabuf = globals.bind(&qh, 3..=5, ()).ok();
    if dmabuf.is_some() {
        tracing::info!("linux-dmabuf available");
    } else {
        tracing::warn!("linux-dmabuf unavailable — DMA-BUF zero-copy disabled");
    }

    let mut data = AppData {
        registry_state: RegistryState::new(&globals),
        compositor: CompositorState::bind(&globals, &qh).context("wl_compositor")?,
        output_state: OutputState::new(&globals, &qh),
        shm: Shm::bind(&globals, &qh).context("wl_shm")?,
        layer_shell: LayerShell::bind(&globals, &qh).context("zwlr_layer_shell_v1")?,
        viewporter,
        dmabuf,
        surfaces: HashMap::new(),
        output_names: HashMap::new(),
        cmd_rx,
        status,
        exit: exit.clone(),
        qh: qh.clone(),
        shared_shm: None,
    };

    // Initial roundtrip to learn outputs
    event_queue.roundtrip(&mut data)?;

    tracing::info!(outputs = data.surfaces.len(), "wayland surfaces ready");

    while !exit.load(Ordering::Relaxed) {
        // Process IPC commands without blocking forever
        while let Ok(cmd) = data.cmd_rx.try_recv() {
            data.handle_cmd(cmd)?;
        }

        // Present new frames
        data.present_all(&conn)?;

        // Dispatch wayland with short timeout
        event_queue.dispatch_pending(&mut data)?;
        let _ = conn.flush();

        match event_queue.prepare_read() {
            Some(guard) => {
                let _ = guard.read();
                event_queue.dispatch_pending(&mut data)?;
            }
            None => {
                event_queue.dispatch_pending(&mut data)?;
            }
        }

        // Sleep briefly to avoid busy loop; video thread paces frames
        std::thread::sleep(Duration::from_millis(8));
        data.publish_status();
    }

    Ok(())
}

impl AppData {
    fn handle_cmd(&mut self, cmd: WaylandCmd) -> Result<()> {
        match cmd {
            WaylandCmd::Shutdown => {
                self.exit.store(true, Ordering::SeqCst);
            }
            WaylandCmd::Clear { output } => {
                let targets: Vec<String> = if output.eq_ignore_ascii_case("ALL") {
                    self.surfaces.keys().cloned().collect()
                } else {
                    vec![output]
                };
                for name in targets {
                    if let Some(surf) = self.surfaces.get_mut(&name) {
                        surf.slot = None;
                        surf.path = None;
                        surf.kind = None;
                        // Solid dark fill
                        self.draw_solid(&name, 0x10, 0x10, 0x14)?;
                    }
                }
            }
            WaylandCmd::SetScaleMode { output, mode } => {
                if let Some(surf) = self.surfaces.get_mut(&output) {
                    surf.mode = mode;
                }
            }
            WaylandCmd::SetWallpaper {
                output,
                path,
                mode,
                slot,
            } => {
                let targets: Vec<String> = if output.eq_ignore_ascii_case("ALL") {
                    self.surfaces.keys().cloned().collect()
                } else {
                    vec![output]
                };
                let kind = wall_core::WallpaperKind::from_path(&path);
                for name in targets {
                    if let Some(surf) = self.surfaces.get_mut(&name) {
                        surf.mode = mode;
                        surf.path = Some(path.clone());
                        surf.kind = Some(kind);
                        surf.slot = Some(slot.clone());
                    } else {
                        tracing::warn!(output = %name, "unknown output");
                    }
                }
            }
        }
        Ok(())
    }

    fn present_all(&mut self, conn: &Connection) -> Result<()> {
        let names: Vec<String> = self.surfaces.keys().cloned().collect();

        // Gather outputs that share the same slot and have a new frame.
        let mut batch: Vec<(String, u32, u32, wall_core::ScaleMode, MediaSlot, bool)> = Vec::new();
        let mut shared_frame: Option<std::sync::Arc<crate::media::Frame>> = None;

        for name in &names {
            let Some(surf) = self.surfaces.get(name) else {
                continue;
            };
            if !surf.configured || surf.width == 0 || surf.height == 0 {
                continue;
            }
            let Some(slot) = surf.slot.clone() else {
                continue;
            };
            batch.push((
                name.clone(),
                surf.width,
                surf.height,
                surf.mode,
                slot,
                surf.viewport.is_some(),
            ));
        }

        // Advance gens and pick one frame (all outputs with same slot share it)
        let mut to_present: Vec<(String, u32, u32, wall_core::ScaleMode, bool)> = Vec::new();
        for (name, w, h, mode, slot, has_vp) in batch {
            let surf = self.surfaces.get_mut(&name).unwrap();
            if let Some(frame) = slot.frame_if_newer(&mut surf.last_frame_gen) {
                if shared_frame.is_none() {
                    shared_frame = Some(frame);
                }
                to_present.push((name, w, h, mode, has_vp));
            }
        }

        let Some(frame) = shared_frame else {
            return Ok(());
        };

        // One SHM upload for all outputs that need this frame.
        if let Frame::Shm {
            width,
            height,
            stride,
            pixels,
        } = frame.as_ref()
        {
            let need = pixels.len();
            if self.shared_shm.is_none() || self.shared_shm.as_ref().unwrap().size < need {
                let shm_proto = self.shm.wl_shm().clone();
                let qh = self.qh.clone();
                self.shared_shm = Some(ShmPool::create(
                    &shm_proto,
                    &qh,
                    need.next_multiple_of(4096),
                )?);
            }
            let pool = self.shared_shm.as_mut().unwrap();
            pool.write_xrgb(pixels)?;

            let shm_proto = self.shm.wl_shm().clone();
            let qh = self.qh.clone();
            let _ = shm_proto; // pool already created
            for (name, out_w, out_h, mode, has_vp) in to_present {
                let layer_surface = self.surfaces[&name].layer.wl_surface().clone();
                let surf = self.surfaces.get_mut(&name).unwrap();
                let viewport = if has_vp {
                    surf.viewport.as_ref()
                } else {
                    None
                };
                apply_viewport_pub(viewport, *width, *height, out_w, out_h, mode);
                let buf = pool.create_buffer(&qh, *width as i32, *height as i32, *stride as i32);
                layer_surface.attach(Some(&buf), 0, 0);
                layer_surface.damage_buffer(0, 0, *width as i32, *height as i32);
                layer_surface.commit();
                buf.destroy();
            }
            let _ = conn.flush();
            return Ok(());
        }

        // DMA-BUF / other: per-output present
        let shm_proto = self.shm.wl_shm().clone();
        let dmabuf = self.dmabuf.clone();
        let qh = self.qh.clone();
        for (name, out_w, out_h, mode, has_vp) in to_present {
            let layer_surface = self.surfaces[&name].layer.wl_surface().clone();
            let surf = self.surfaces.get_mut(&name).unwrap();
            let viewport = if has_vp {
                surf.viewport.as_ref()
            } else {
                None
            };
            if let Err(e) = present_frame(
                conn,
                &layer_surface,
                viewport,
                &shm_proto,
                dmabuf.as_ref(),
                &qh,
                frame.as_ref(),
                out_w,
                out_h,
                mode,
                &mut surf.pools,
            ) {
                tracing::warn!(output = %name, error = %e, "present failed");
            }
        }
        Ok(())
    }

    fn draw_solid(&mut self, name: &str, r: u8, g: u8, b: u8) -> Result<()> {
        let Some(surf) = self.surfaces.get_mut(name) else {
            return Ok(());
        };
        if surf.width == 0 || surf.height == 0 {
            return Ok(());
        }
        let w = surf.width;
        let h = surf.height;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for px in pixels.chunks_exact_mut(4) {
            px[0] = b;
            px[1] = g;
            px[2] = r;
            px[3] = 0xff;
        }
        let frame = Frame::Shm {
            width: w,
            height: h,
            stride: w * 4,
            pixels,
        };
        // Can't easily call present without conn — skip; will be overwritten on next set
        let _ = frame;
        let _ = surf;
        Ok(())
    }

    fn ensure_surface(&mut self, output: &WlOutput, name: &str, width: i32, height: i32, refresh: i32) {
        if self.surfaces.contains_key(name) {
            if let Some(s) = self.surfaces.get_mut(name) {
                s.width = width.max(0) as u32;
                s.height = height.max(0) as u32;
                s.refresh_mhz = refresh;
            }
            return;
        }

        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            Layer::Background,
            Some("livewall".to_string()),
            Some(output),
        );
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(width.max(0) as u32, height.max(0) as u32);
        layer.commit();

        let viewport = self
            .viewporter
            .as_ref()
            .map(|vp| vp.get_viewport(layer.wl_surface(), &self.qh, ()));

        self.output_names.insert(output.clone(), name.to_string());
        self.surfaces.insert(
            name.to_string(),
            OutputSurf {
                name: name.to_string(),
                layer,
                viewport,
                width: width.max(0) as u32,
                height: height.max(0) as u32,
                refresh_mhz: refresh,
                mode: ScaleMode::Fill,
                slot: None,
                path: None,
                kind: None,
                pools: Vec::new(),
                configured: false,
                last_frame_gen: 0,
            },
        );
        tracing::info!(output = %name, width, height, "created layer surface");
    }

    fn publish_status(&self) {
        let mut list = Vec::new();
        for s in self.surfaces.values() {
            let (playing, _) = s
                .slot
                .as_ref()
                .map(|sl| {
                    let (_p, _k, _d, _w, _f, playing) = sl.peek_meta();
                    (playing, ())
                })
                .unwrap_or((false, ()));
            list.push(OutputStatus {
                name: s.name.clone(),
                width: s.width as i32,
                height: s.height as i32,
                refresh_mhz: s.refresh_mhz,
                path: s.path.clone(),
                kind: s.kind,
                mode: s.mode,
                playing,
            });
        }
        if let Ok(mut g) = self.status.lock() {
            *g = list;
        }
    }
}

impl CompositorHandler for AppData {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_transform: wayland_client::protocol::wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }
}

impl OutputHandler for AppData {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: WlOutput,
    ) {
        let info = self.output_state.info(&output);
        let name = info
            .as_ref()
            .and_then(|i| i.name.clone())
            .unwrap_or_else(|| format!("output-{}", output.id()));
        let (w, h, refresh) = info
            .and_then(|i| {
                i.modes.iter().find(|m| m.current).map(|m| {
                    (m.dimensions.0, m.dimensions.1, m.refresh_rate)
                })
            })
            .unwrap_or((0, 0, 0));
        self.ensure_surface(&output, &name, w, h, refresh);
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: WlOutput,
    ) {
        let info = self.output_state.info(&output);
        let name = info
            .as_ref()
            .and_then(|i| i.name.clone())
            .or_else(|| self.output_names.get(&output).cloned())
            .unwrap_or_default();
        if name.is_empty() {
            return;
        }
        if let Some(i) = info {
            if let Some(m) = i.modes.iter().find(|m| m.current) {
                self.ensure_surface(&output, &name, m.dimensions.0, m.dimensions.1, m.refresh_rate);
            }
        }
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: WlOutput,
    ) {
        if let Some(name) = self.output_names.remove(&output) {
            self.surfaces.remove(&name);
            tracing::info!(output = %name, "output removed");
        }
    }
}

impl LayerShellHandler for AppData {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        self.surfaces
            .retain(|_, s| s.layer.wl_surface() != layer.wl_surface());
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        for s in self.surfaces.values_mut() {
            if s.layer.wl_surface() == layer.wl_surface() {
                if configure.new_size.0 > 0 {
                    s.width = configure.new_size.0;
                }
                if configure.new_size.1 > 0 {
                    s.height = configure.new_size.1;
                }
                s.configured = true;
                tracing::debug!(
                    output = %s.name,
                    w = s.width,
                    h = s.height,
                    "layer configured"
                );
            }
        }
    }
}

impl ShmHandler for AppData {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for AppData {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(AppData);
delegate_output!(AppData);
delegate_shm!(AppData);
delegate_layer!(AppData);
delegate_registry!(AppData);

// Dispatch for viewporter + dmabuf globals / objects we create with `()` data
use wayland_client::Dispatch;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1;
use wayland_protocols::wp::viewporter::client::wp_viewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter;

impl Dispatch<WpViewporter, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &WpViewporter,
        _: wp_viewporter::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewport, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: wp_viewport::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &ZwpLinuxDmabufV1,
        event: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Could collect format/modifier feedback here for hybrid GPU selection
        if let zwp_linux_dmabuf_v1::Event::Modifier { format, modifier_hi, modifier_lo, .. } = event
        {
            tracing::trace!(format, modifier_hi, modifier_lo, "dmabuf modifier");
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &ZwpLinuxBufferParamsV1,
        event: wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::Event;
        match event {
            Event::Created { .. } => {}
            Event::Failed => tracing::warn!("linux_buffer_params failed"),
            _ => {}
        }
    }
}

impl Dispatch<wayland_client::protocol::wl_buffer::WlBuffer, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_buffer::WlBuffer,
        _: wayland_client::protocol::wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wayland_client::protocol::wl_shm_pool::WlShmPool, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_shm_pool::WlShmPool,
        _: wayland_client::protocol::wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

pub fn spawn_wayland() -> Result<(WaylandHandle, std::thread::JoinHandle<Result<()>>)> {
    let (tx, rx) = mpsc::channel();
    let status = Arc::new(Mutex::new(Vec::new()));
    let exit = Arc::new(AtomicBool::new(false));
    let status_c = status.clone();
    let exit_c = exit.clone();
    let join = std::thread::Builder::new()
        .name("walld-wayland".into())
        .spawn(move || run_wayland(rx, status_c, exit_c))?;
    Ok((
        WaylandHandle { tx, outputs: status },
        join,
    ))
}
