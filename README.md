# livewall

Efficient Wayland wallpaper daemon written in Rust. Plays high-quality video and still images with low CPU/RAM by using VA-API hardware decode when available.

**Supported compositors:** wlroots-based only (`zwlr_layer_shell_v1`) — Hyprland, Sway, niri, river, etc. Not GNOME.

## Features

- Video wallpapers (MP4/MKV/WebM, …) with VA-API decode
- Still images (PNG/JPEG/WebP) and animated GIF/WebP
- Per-output wallpapers and scale modes (`fill` / `fit` / `stretch` / `center`)
- Zero-copy **DMA-BUF** present when the GPU/compositor allow it
- **Hybrid GPU** safe path (Intel + NVIDIA): VA-API decode + SHM present so dGPU-wired HDMI works
- Daemon + CLI (`livewall` / `livewallctl`), Unix-socket IPC
- Pause on Hyprland fullscreen (best-effort), optional battery FPS cap
- Config at `~/.config/livewall/config.toml`

## Build

Dependencies (Arch example):

```bash
sudo pacman -S --needed rust ffmpeg libva mesa wayland wayland-protocols pkgconf gcc
```

```bash
cargo build --release -p livewall -p livewallctl
install -Dm755 target/release/livewall  ~/.local/bin/livewall
install -Dm755 target/release/livewallctl ~/.local/bin/livewallctl
```

## Usage

```bash
# start daemon (or enable the systemd user unit — see below)
livewall &

livewallctl set ALL ~/Videos/loop.mp4 --mode fill
livewallctl set eDP-1 ~/Pictures/wall.png --mode fill
livewallctl status
livewallctl pause
livewallctl resume
livewallctl shutdown
```

Example config: [`config/config.example.toml`](config/config.example.toml)

### systemd (user)

```bash
# ~/.config/systemd/user/livewall.service
[Unit]
Description=livewall Wayland wallpaper daemon
PartOf=graphical-session.target
After=graphical-session.target

[Service]
ExecStart=%h/.local/bin/livewall --hwdec auto
Restart=on-failure

[Install]
WantedBy=graphical-session.target
```

```bash
systemctl --user enable --now livewall.service
```

### niri

Background wallpapers need a layer rule (DMS/quickshell may also own the backdrop — hide or disable its wallpaper surface if it covers livewall):

```kdl
layer-rule {
    match namespace="^livewall"
    place-within-backdrop true
}
```

More compositor notes: [`docs/compositors.md`](docs/compositors.md)

## Efficiency

| Path | When |
|------|------|
| VA-API → DMA-BUF | Single-GPU, compositor accepts the buffer |
| VA-API → SHM | Hybrid Intel/NVIDIA (or `LIVEWALL_FORCE_SHM=1`) |
| Software decode | VA-API unavailable (warning logged) |

Override render node: `LIVEWALL_RENDER_NODE=/dev/dri/renderD129`

`efficiency_mode = true` in config (or `--efficiency-mode`) refuses software fallback.

## Project layout

| Crate | Role |
|-------|------|
| `livewall` | Daemon (Wayland + decode) |
| `livewallctl` | CLI |
| `livewall-core` | Shared config + IPC types |

Native FFmpeg bridge: `crates/livewall/native/wall_decode.c`

## License

MIT
