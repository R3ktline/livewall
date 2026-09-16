# Compositor setup

`walld` needs `zwlr_layer_shell_v1`, `wl_shm`, and ideally `zwp_linux_dmabuf_v1` + `wp_viewporter`.

## Hyprland

```ini
# hyprland.conf
exec-once = walld
```

Optional: ensure background layers are not killed by wallpaper tools (`hyprpaper` / `swww` can fight for the background — pick one).

Fullscreen auto-pause uses the Hyprland IPC socket when `HYPRLAND_INSTANCE_SIGNATURE` is set.

## Sway

```bash
# ~/.config/sway/config
exec walld
```

## niri

niri may require a layer rule so background layer-shell surfaces are visible:

```kdl
layer-rule {
    match namespace="livewall-*"
    place-within-backdrop true
}
```

(Exact knobs vary by niri version — if the wallpaper is invisible, check layer-shell / backdrop docs for your release.)

```kdl
spawn-at-startup "walld"
```

## river

```bash
riverctl spawn walld
```

## Checking the decode path

```bash
wallctl status
```

Look for `decode path: vaapi-dmabuf`. If you see `soft`, hardware decode or DMA-BUF export failed — check `vainfo`, drivers, and `WALLD_RENDER_NODE`.
