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
- Easy wallpaper switching: `random`, `next`, `prev`, and folder cycling
- Remembers your wallpaper across restarts (`--save` / `save`) and can **auto-start on login**
- Pause on Hyprland fullscreen (best-effort), optional battery FPS cap
- Config at `~/.config/livewall/config.toml`

## Install

Dependencies (Arch example):

```bash
sudo pacman -S --needed rust ffmpeg libva mesa wayland wayland-protocols libxkbcommon pkgconf gcc
```

Build and install both binaries:

```bash
cargo build --release -p livewall -p livewallctl
install -Dm755 target/release/livewall    ~/.local/bin/livewall
install -Dm755 target/release/livewallctl ~/.local/bin/livewallctl
```

Make sure `~/.local/bin` is on your `PATH`.

## Quick start

```bash
livewall &                                        # start the daemon

livewallctl set ALL ~/Videos/loop.mp4 --mode fill # video on every output
livewallctl set eDP-1 ~/Pictures/wall.png         # image on one output
livewallctl status                                # show outputs / decode path
```

## Changing wallpapers

`livewallctl` talks to the running daemon over a Unix socket.

| Command | What it does |
|---------|--------------|
| `set <OUTPUT> <PATH> [--mode M] [--save]` | Set a specific image/video (`ALL` targets every output) |
| `random <DIR> [--output O] [--mode M] [--save]` | Pick a random wallpaper from a folder |
| `next <DIR> [--output O] [--save]` | Next wallpaper in a folder (alphabetical, wraps) |
| `prev <DIR> [--output O] [--save]` | Previous wallpaper in a folder |
| `list` | Print output names (handy for scripts) |
| `clear <OUTPUT>` | Clear a wallpaper (`ALL` for every output) |
| `save` | Persist the currently shown wallpapers to the config |
| `pause` / `resume` | Pause/resume playback |
| `hwdec <auto\|vaapi\|vulkan\|soft>` | Change decode preference at runtime |
| `status [--json]` | Show daemon and per-output status |
| `shutdown` | Ask the daemon to exit |

`--output` defaults to `ALL`. Scale modes are `fill` (default), `fit`, `stretch`, `center`.

Examples:

```bash
livewallctl random ~/Wallpapers                   # surprise me
livewallctl next ~/Wallpapers                      # step through a folder
livewallctl set ALL ~/Pictures/wall.png --save     # set and remember it

# Cycle wallpapers on a timer or a keybind
watch -n 900 'livewallctl next ~/Wallpapers'       # new wallpaper every 15 min
```

## Remembering wallpapers across restarts

By default a `set` only affects the running session. Persist it so the daemon
restores it on the next start:

- `livewallctl set ALL ~/Pictures/wall.png --save` — set and save in one step
- `livewallctl save` — save whatever is currently displayed (works after `random`/`next`)

Saved wallpapers are written to `~/.config/livewall/config.toml` and reapplied
automatically when the daemon starts.

## Auto-start on login

Install and enable a systemd **user** service so the daemon (and your saved
wallpaper) start automatically when you log into your graphical session:

```bash
livewallctl autostart enable            # install + enable the user service
livewallctl autostart enable --hwdec vaapi   # pin a decode preference
livewallctl autostart status            # show install / enabled / active state
livewallctl autostart disable           # stop, disable, and remove it
```

`autostart enable` writes `~/.config/systemd/user/livewall.service` pointing at
the installed `livewall` binary, then enables it (and starts it if a user
session bus is available). Combine it with `--save` so your wallpaper is back
the moment you log in.

If you prefer to manage the unit yourself, it is equivalent to:

```ini
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

Compositor-specific autostart (`exec-once`, `spawn-at-startup`, …) and layer
rules for niri are covered in [`docs/compositors.md`](docs/compositors.md).

## Efficiency

| Path | When |
|------|------|
| VA-API → DMA-BUF | Single-GPU, compositor accepts the buffer |
| VA-API → SHM | Hybrid Intel/NVIDIA (or `LIVEWALL_FORCE_SHM=1`) |
| Software decode | VA-API unavailable (warning logged) |

Override render node: `LIVEWALL_RENDER_NODE=/dev/dri/renderD129`

`efficiency_mode = true` in config (or `--efficiency-mode`) refuses software fallback.

Example config: [`config/config.example.toml`](config/config.example.toml)

## Project layout

| Crate | Role |
|-------|------|
| `livewall` | Daemon (Wayland + decode) |
| `livewallctl` | CLI |
| `livewall-core` | Shared config + IPC types |

Native FFmpeg bridge: `crates/livewall/native/livewall_decode.c`

## License

MIT
