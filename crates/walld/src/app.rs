use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::net::UnixListener;
use wall_core::{
    read_message, socket_path, write_message, Config, Request, Response, ScaleMode, StatusInfo,
};

use crate::media::{self, DecodePath, MediaSlot, PlayerHandle};
use crate::metrics;
use crate::power;
use crate::wayland::{self, WaylandCmd, WaylandHandle};

pub struct App {
    cfg: Arc<RwLock<Config>>,
    wayland: WaylandHandle,
    players: Mutex<HashMap<String, PlayerHandle>>,
    slots: Mutex<HashMap<String, MediaSlot>>,
    paused: Arc<AtomicBool>,
    auto_paused: AtomicBool,
    decode_path: Mutex<DecodePath>,
    warning: Mutex<Option<String>>,
    exit: Arc<AtomicBool>,
}

impl App {
    pub fn run(cfg: Arc<RwLock<Config>>) -> Result<()> {
        let (wayland, wl_join) = wayland::spawn_wayland()?;
        let exit = Arc::new(AtomicBool::new(false));
        let app = Arc::new(Self {
            cfg: cfg.clone(),
            wayland,
            players: Mutex::new(HashMap::new()),
            slots: Mutex::new(HashMap::new()),
            paused: Arc::new(AtomicBool::new(false)),
            auto_paused: AtomicBool::new(false),
            decode_path: Mutex::new(DecodePath::None),
            warning: Mutex::new(None),
            exit: exit.clone(),
        });

        // Apply defaults after a short wait for outputs
        {
            let app_c = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                if let Err(e) = app_c.apply_config_defaults() {
                    tracing::warn!(error = %e, "failed applying default wallpapers");
                }
            });
        }

        // Power / fullscreen pause loop
        {
            let app_c = app.clone();
            std::thread::spawn(move || loop {
                if app_c.exit.load(Ordering::Relaxed) {
                    break;
                }
                app_c.tick_power_policy();
                std::thread::sleep(Duration::from_millis(500));
            });
        }

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let app_ipc = app.clone();
        let ipc_result = rt.block_on(async move { app_ipc.serve_ipc().await });

        let _ = app.wayland.tx.send(WaylandCmd::Shutdown);
        exit.store(true, Ordering::SeqCst);
        let _ = wl_join.join();
        ipc_result
    }

    fn apply_config_defaults(&self) -> Result<()> {
        let cfg = self.cfg.read().unwrap().clone();
        if let Some(path) = &cfg.default_path {
            self.set_wallpaper("ALL", path.clone(), Some(cfg.default_mode))?;
        }
        for (name, out) in &cfg.outputs {
            if let Some(path) = &out.path {
                self.set_wallpaper(name, path.clone(), out.mode.or(Some(cfg.default_mode)))?;
            }
        }
        Ok(())
    }

    fn tick_power_policy(&self) {
        let cfg = self.cfg.read().unwrap().clone();

        // Manual pause always wins — only auto-toggle when not manually paused
        let manually =
            self.paused.load(Ordering::Relaxed) && !self.auto_paused.load(Ordering::Relaxed);
        if manually {
            return;
        }

        let mut auto = false;
        if cfg.pause.on_fullscreen && power::hyprland_fullscreen_focused() {
            auto = true;
        }
        if cfg.pause.on_dpms {
            if let Ok(outs) = self.wayland.outputs.lock() {
                if outs.is_empty() {
                    auto = true;
                }
            }
        }

        if auto {
            if !self.paused.load(Ordering::Relaxed) {
                self.paused.store(true, Ordering::SeqCst);
                self.auto_paused.store(true, Ordering::SeqCst);
                tracing::debug!("auto-pause (fullscreen/dpms)");
            }
        } else if self.auto_paused.load(Ordering::Relaxed) {
            self.paused.store(false, Ordering::SeqCst);
            self.auto_paused.store(false, Ordering::SeqCst);
            tracing::debug!("auto-resume");
        }
    }

    fn set_wallpaper(
        &self,
        output: &str,
        path: PathBuf,
        mode: Option<ScaleMode>,
    ) -> Result<()> {
        media::validate_path(&path)?;
        let cfg = self.cfg.read().unwrap().clone();
        let mode = mode.unwrap_or(cfg.default_mode);

        let mut max_fps = cfg.max_fps;
        if cfg.pause.half_fps_on_battery && power::on_battery() {
            if max_fps == 0 {
                max_fps = 30;
            } else {
                max_fps = (max_fps / 2).max(1);
            }
        }

        let targets: Vec<String> = if output.eq_ignore_ascii_case("ALL") {
            self.wayland
                .outputs
                .lock()
                .unwrap()
                .iter()
                .map(|o| o.name.clone())
                .collect()
        } else {
            vec![output.to_string()]
        };

        if targets.is_empty() && output.eq_ignore_ascii_case("ALL") {
            // Outputs not ready — stash as default
            self.cfg.write().unwrap().default_path = Some(path.clone());
            self.cfg.write().unwrap().default_mode = mode;
            bail!("no outputs yet; saved as default and will apply shortly");
        }

        // One shared player/slot when ALL and same file — share slot across outputs
        let slot = MediaSlot::default();
        let player = media::spawn_player(
            &path,
            slot.clone(),
            cfg.hwdec,
            cfg.efficiency_mode,
            max_fps,
            self.paused.clone(),
        )?;

        {
            let mut players = self.players.lock().unwrap();
            let mut slots = self.slots.lock().unwrap();
            if output.eq_ignore_ascii_case("ALL") {
                players.clear();
                slots.clear();
                players.insert("ALL".into(), player);
                slots.insert("ALL".into(), slot.clone());
            } else {
                players.insert(output.to_string(), player);
                slots.insert(output.to_string(), slot.clone());
            }
        }

        // Propagate decode meta shortly
        std::thread::sleep(Duration::from_millis(50));
        let (_p, _k, path_dec, warn, _f, _) = slot.peek_meta();
        *self.decode_path.lock().unwrap() = path_dec;
        *self.warning.lock().unwrap() = warn;

        self.wayland
            .tx
            .send(WaylandCmd::SetWallpaper {
                output: output.to_string(),
                path,
                mode,
                slot,
            })
            .context("wayland channel closed")?;
        Ok(())
    }

    async fn serve_ipc(self: Arc<Self>) -> Result<()> {
        let path = socket_path();
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("bind {}", path.display()))?;
        tracing::info!(path = %path.display(), "IPC listening");

        loop {
            if self.exit.load(Ordering::Relaxed) {
                break;
            }
            let (stream, _) = listener.accept().await?;
            let app = self.clone();
            tokio::spawn(async move {
                if let Err(e) = app.handle_client(stream).await {
                    tracing::debug!(error = %e, "ipc client error");
                }
            });
        }
        Ok(())
    }

    async fn handle_client(self: Arc<Self>, stream: tokio::net::UnixStream) -> Result<()> {
        let std_stream = stream.into_std()?;
        std_stream.set_nonblocking(false)?;
        let mut stream = std_stream;
        let req: Request = read_message(&mut stream)?;
        let resp = self.dispatch(req);
        write_message(&mut stream, &resp)?;
        if matches!(resp, Response::Ok { .. }) {
            // shutdown handled in dispatch
        }
        Ok(())
    }

    fn dispatch(&self, req: Request) -> Response {
        match req {
            Request::Ping => Response::Pong,
            Request::Shutdown => {
                self.exit.store(true, Ordering::SeqCst);
                let _ = self.wayland.tx.send(WaylandCmd::Shutdown);
                Response::Ok {
                    message: Some("shutting down".into()),
                }
            }
            Request::Pause => {
                self.paused.store(true, Ordering::SeqCst);
                self.auto_paused.store(false, Ordering::SeqCst);
                Response::Ok {
                    message: Some("paused".into()),
                }
            }
            Request::Resume => {
                self.paused.store(false, Ordering::SeqCst);
                self.auto_paused.store(false, Ordering::SeqCst);
                Response::Ok {
                    message: Some("resumed".into()),
                }
            }
            Request::SetHwdec { hwdec } => {
                self.cfg.write().unwrap().hwdec = hwdec;
                Response::Ok {
                    message: Some(format!("hwdec set to {}", hwdec.as_str())),
                }
            }
            Request::Clear { output } => {
                {
                    let mut players = self.players.lock().unwrap();
                    let mut slots = self.slots.lock().unwrap();
                    if output.eq_ignore_ascii_case("ALL") {
                        players.clear();
                        slots.clear();
                    } else {
                        players.remove(&output);
                        slots.remove(&output);
                    }
                }
                let _ = self.wayland.tx.send(WaylandCmd::Clear { output });
                Response::Ok {
                    message: Some("cleared".into()),
                }
            }
            Request::Set { output, path, mode } => match self.set_wallpaper(&output, path, mode) {
                Ok(()) => Response::Ok {
                    message: Some("ok".into()),
                },
                Err(e) => Response::Error {
                    message: format!("{e:#}"),
                },
            },
            Request::Status => Response::Status(self.status_info()),
        }
    }

    fn status_info(&self) -> StatusInfo {
        let cfg = self.cfg.read().unwrap().clone();
        let outputs = self.wayland.outputs.lock().unwrap().clone();

        let (decode_path, warning, fps) = {
            let slots = self.slots.lock().unwrap();
            let mut decode = DecodePath::None;
            let mut warn = self.warning.lock().unwrap().clone();
            let mut fps = 0.0f32;
            for s in slots.values() {
                let (_p, _k, path_dec, w, f, _) = s.peek_meta();
                if path_dec != DecodePath::None {
                    decode = path_dec;
                }
                if w.is_some() {
                    warn = w;
                }
                fps = fps.max(f);
            }
            if decode == DecodePath::None {
                decode = *self.decode_path.lock().unwrap();
            }
            (decode.as_str().to_string(), warn, fps)
        };

        StatusInfo {
            paused: self.paused.load(Ordering::Relaxed),
            hwdec: cfg.hwdec.as_str().to_string(),
            decode_path,
            efficiency_warning: warning,
            fps,
            rss_bytes: metrics::rss_bytes(),
            outputs,
        }
    }
}
