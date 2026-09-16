use std::fs;
use std::path::Path;

/// True when a battery is discharging.
pub fn on_battery() -> bool {
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        let ty = fs::read_to_string(path.join("type")).unwrap_or_default();
        if !ty.trim().eq_ignore_ascii_case("Battery") {
            continue;
        }
        let status = fs::read_to_string(path.join("status")).unwrap_or_default();
        if status.trim().eq_ignore_ascii_case("Discharging") {
            return true;
        }
    }
    false
}

/// Hyprland fullscreen hint via HYPRLAND_INSTANCE_SIGNATURE socket (best-effort).
pub fn hyprland_fullscreen_focused() -> bool {
    let Ok(sig) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") else {
        return false;
    };
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let candidates = [
        Path::new(&runtime).join("hypr").join(&sig).join(".socket.sock"),
        Path::new(&runtime).join(format!("hypr/{sig}/.socket.sock")),
    ];
    let sock = candidates.into_iter().find(|p| p.exists());
    let Some(sock) = sock else {
        return false;
    };
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    let Ok(mut stream) = UnixStream::connect(&sock) else {
        return false;
    };
    if stream.write_all(b"activewindow").is_err() {
        return false;
    }
    let mut buf = String::new();
    if stream.read_to_string(&mut buf).is_err() {
        return false;
    }
    for line in buf.lines() {
        if let Some(rest) = line.strip_prefix("fullscreen:") {
            return !matches!(rest.trim(), "0" | "false");
        }
    }
    false
}
