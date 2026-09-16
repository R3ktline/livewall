use std::fs;
use std::io::{BufReader, BufWriter};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use livewall_core::{
    read_message, socket_path, write_message, HwDecPreference, Request, Response, ScaleMode,
    StatusInfo,
};

/// Image / video extensions livewall can display, used when picking from a folder.
const MEDIA_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "mp4", "mkv", "webm", "mov", "avi", "m4v", "ts",
];

#[derive(Parser, Debug)]
#[command(name = "livewallctl", about = "Control the livewall wallpaper daemon")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Check daemon connectivity
    Ping,
    /// Set wallpaper on an output (`ALL` for every output)
    Set {
        /// Output name or ALL
        output: String,
        /// Path to image or video
        path: PathBuf,
        #[arg(long, value_parser = parse_mode)]
        mode: Option<ScaleMode>,
        /// Also persist this wallpaper so it is restored on the next start
        #[arg(long)]
        save: bool,
    },
    /// Set a random wallpaper from a folder
    Random {
        /// Folder to pick a wallpaper from
        dir: PathBuf,
        /// Output name or ALL (default: ALL)
        #[arg(long, default_value = "ALL")]
        output: String,
        #[arg(long, value_parser = parse_mode)]
        mode: Option<ScaleMode>,
        /// Persist the chosen wallpaper for the next start
        #[arg(long)]
        save: bool,
    },
    /// Switch to the next wallpaper in a folder (alphabetical)
    Next {
        /// Folder to cycle through
        dir: PathBuf,
        #[arg(long, default_value = "ALL")]
        output: String,
        #[arg(long, value_parser = parse_mode)]
        mode: Option<ScaleMode>,
        /// Persist the chosen wallpaper for the next start
        #[arg(long)]
        save: bool,
    },
    /// Switch to the previous wallpaper in a folder (alphabetical)
    Prev {
        /// Folder to cycle through
        dir: PathBuf,
        #[arg(long, default_value = "ALL")]
        output: String,
        #[arg(long, value_parser = parse_mode)]
        mode: Option<ScaleMode>,
        /// Persist the chosen wallpaper for the next start
        #[arg(long)]
        save: bool,
    },
    /// Clear wallpaper on an output
    Clear { output: String },
    /// List output names (one per line)
    List,
    /// Persist the currently displayed wallpapers so they reload on next start
    Save,
    /// Pause playback
    Pause,
    /// Resume playback
    Resume,
    /// Show daemon / output status
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Ask daemon to exit
    Shutdown,
    /// Change hardware decode preference at runtime
    Hwdec {
        #[arg(value_parser = parse_hwdec)]
        prefer: HwDecPreference,
    },
    /// Manage the systemd user service that starts livewall on login
    Autostart {
        #[command(subcommand)]
        action: AutostartAction,
    },
}

#[derive(Subcommand, Debug)]
enum AutostartAction {
    /// Install and enable the systemd user service
    Enable {
        /// Hardware decode preference the service starts with
        #[arg(long, default_value = "auto", value_parser = parse_hwdec)]
        hwdec: HwDecPreference,
    },
    /// Disable and remove the systemd user service
    Disable,
    /// Show whether autostart is installed / enabled
    Status,
}

fn parse_mode(s: &str) -> Result<ScaleMode, String> {
    ScaleMode::parse(s).ok_or_else(|| format!("invalid scale mode: {s}"))
}

fn parse_hwdec(s: &str) -> Result<HwDecPreference, String> {
    HwDecPreference::parse(s).ok_or_else(|| format!("invalid hwdec: {s}"))
}

fn connect() -> Result<UnixStream> {
    let path = socket_path();
    let stream = UnixStream::connect(&path)
        .with_context(|| format!("connecting to {} (is livewall running?)", path.display()))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    Ok(stream)
}

fn roundtrip(req: Request) -> Result<Response> {
    let stream = connect()?;
    let mut writer = BufWriter::new(&stream);
    write_message(&mut writer, &req)?;
    let mut reader = BufReader::new(&stream);
    let resp: Response = read_message(&mut reader)?;
    Ok(resp)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Ping => print_response(roundtrip(Request::Ping)?),
        Cmd::Set {
            output,
            path,
            mode,
            save,
        } => set_wallpaper(output, path, mode, save),
        Cmd::Random {
            dir,
            output,
            mode,
            save,
        } => random(dir, output, mode, save),
        Cmd::Next {
            dir,
            output,
            mode,
            save,
        } => cycle(dir, output, mode, save, true),
        Cmd::Prev {
            dir,
            output,
            mode,
            save,
        } => cycle(dir, output, mode, save, false),
        Cmd::Clear { output } => print_response(roundtrip(Request::Clear { output })?),
        Cmd::List => list_outputs(),
        Cmd::Save => print_response(roundtrip(Request::Save)?),
        Cmd::Pause => print_response(roundtrip(Request::Pause)?),
        Cmd::Resume => print_response(roundtrip(Request::Resume)?),
        Cmd::Status { json } => print_status(fetch_status()?, json),
        Cmd::Shutdown => print_response(roundtrip(Request::Shutdown)?),
        Cmd::Hwdec { prefer } => print_response(roundtrip(Request::SetHwdec { hwdec: prefer })?),
        Cmd::Autostart { action } => match action {
            AutostartAction::Enable { hwdec } => autostart_enable(hwdec),
            AutostartAction::Disable => autostart_disable(),
            AutostartAction::Status => autostart_status(),
        },
    }
}

fn set_wallpaper(output: String, path: PathBuf, mode: Option<ScaleMode>, save: bool) -> Result<()> {
    let path = path.canonicalize().unwrap_or(path);
    print_response(roundtrip(Request::Set {
        output,
        path,
        mode,
        save,
    })?)
}

fn random(dir: PathBuf, output: String, mode: Option<ScaleMode>, save: bool) -> Result<()> {
    let files = list_media(&dir)?;
    if files.is_empty() {
        bail!("no media files found in {}", dir.display());
    }
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as usize;
    let mut idx = seed % files.len();
    // Avoid re-picking the wallpaper that is already showing when possible.
    if files.len() > 1 {
        if let Some(cur) = current_path(&output, &fetch_status().ok()) {
            if files[idx] == cur {
                idx = (idx + 1) % files.len();
            }
        }
    }
    let chosen = files[idx].clone();
    println!("→ {}", chosen.display());
    set_wallpaper(output, chosen, mode, save)
}

fn cycle(
    dir: PathBuf,
    output: String,
    mode: Option<ScaleMode>,
    save: bool,
    forward: bool,
) -> Result<()> {
    let files = list_media(&dir)?;
    if files.is_empty() {
        bail!("no media files found in {}", dir.display());
    }
    let n = files.len();
    let cur = current_path(&output, &fetch_status().ok());
    let cur_idx = cur.and_then(|c| files.iter().position(|f| *f == c));
    let next_idx = match cur_idx {
        Some(i) if forward => (i + 1) % n,
        Some(i) => (i + n - 1) % n,
        None => 0,
    };
    let chosen = files[next_idx].clone();
    println!("→ {}", chosen.display());
    set_wallpaper(output, chosen, mode, save)
}

/// Collect displayable media files in `dir`, sorted by path for stable cycling.
fn list_media(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in
        fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?
    {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if MEDIA_EXTS.contains(&ext.as_str()) {
            files.push(path.canonicalize().unwrap_or(path));
        }
    }
    files.sort();
    Ok(files)
}

/// The wallpaper currently shown on `output` (first output for `ALL`).
fn current_path(output: &str, status: &Option<StatusInfo>) -> Option<PathBuf> {
    let status = status.as_ref()?;
    if output.eq_ignore_ascii_case("ALL") {
        status.outputs.iter().find_map(|o| o.path.clone())
    } else {
        status
            .outputs
            .iter()
            .find(|o| o.name == output)
            .and_then(|o| o.path.clone())
    }
}

fn fetch_status() -> Result<StatusInfo> {
    match roundtrip(Request::Status)? {
        Response::Status(info) => Ok(info),
        Response::Error { message } => bail!("{message}"),
        _ => bail!("unexpected response to status request"),
    }
}

fn list_outputs() -> Result<()> {
    let status = fetch_status()?;
    if status.outputs.is_empty() {
        eprintln!("(no outputs)");
    }
    for o in &status.outputs {
        println!("{}", o.name);
    }
    Ok(())
}

fn print_response(resp: Response) -> Result<()> {
    match resp {
        Response::Ok { message } => {
            if let Some(m) = message {
                println!("{m}");
            }
        }
        Response::Pong => println!("pong"),
        Response::Error { message } => bail!("{message}"),
        Response::Status(info) => print_status(info, false)?,
    }
    Ok(())
}

fn print_status(info: StatusInfo, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }
    println!("paused:           {}", info.paused);
    println!("hwdec preference: {}", info.hwdec);
    println!("decode path:      {}", info.decode_path);
    println!("fps:              {:.1}", info.fps);
    println!(
        "rss:              {:.1} MiB",
        info.rss_bytes as f64 / (1024.0 * 1024.0)
    );
    if let Some(w) = &info.efficiency_warning {
        println!("warning:          {w}");
    }
    println!("outputs:");
    for o in &info.outputs {
        let path = o
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".into());
        let kind = o.kind.map(|k| k.as_str()).unwrap_or("-");
        println!(
            "  {}  {}x{}@{:.0}Hz  mode={}  kind={}  playing={}  {}",
            o.name,
            o.width,
            o.height,
            o.refresh_mhz as f32 / 1000.0,
            o.mode.as_str(),
            kind,
            o.playing,
            path
        );
    }
    Ok(())
}

// ---- autostart (systemd user service) ------------------------------------

fn systemd_user_dir() -> Result<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Ok(PathBuf::from(x).join("systemd/user"));
        }
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}

fn unit_path() -> Result<PathBuf> {
    Ok(systemd_user_dir()?.join("livewall.service"))
}

/// Absolute path to the `livewall` daemon, preferring a binary next to this
/// `livewallctl`, then falling back to the name on `PATH`.
fn daemon_binary() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("livewall");
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    "livewall".to_string()
}

fn unit_contents(hwdec: HwDecPreference) -> String {
    format!(
        "[Unit]\n\
Description=livewall Wayland wallpaper daemon\n\
Documentation=https://github.com/r3ktline/livewall\n\
PartOf=graphical-session.target\n\
After=graphical-session.target\n\
\n\
[Service]\n\
Type=simple\n\
ExecStart={bin} --hwdec {hwdec}\n\
ExecReload=/bin/kill -HUP $MAINPID\n\
Restart=on-failure\n\
RestartSec=2\n\
\n\
[Install]\n\
WantedBy=graphical-session.target\n",
        bin = daemon_binary(),
        hwdec = hwdec.as_str(),
    )
}

/// Run `systemctl --user <args>`, returning stdout on success or an error with stderr.
fn systemctl(args: &[&str]) -> Result<String> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("failed to run systemctl (is systemd installed?)")?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if stderr.is_empty() { stdout } else { stderr };
        bail!("{msg}");
    }
}

fn autostart_enable(hwdec: HwDecPreference) -> Result<()> {
    let dir = systemd_user_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let unit = dir.join("livewall.service");
    fs::write(&unit, unit_contents(hwdec)).with_context(|| format!("writing {}", unit.display()))?;
    println!("installed {}", unit.display());

    let _ = systemctl(&["daemon-reload"]);

    // Enable for future logins (only needs the unit files, not a session bus).
    match systemctl(&["enable", "livewall.service"]) {
        Ok(_) => println!("enabled livewall.service — it will start automatically on login"),
        Err(e) => {
            println!("note: could not enable the service: {e}");
            println!("The unit is installed. From a graphical (Wayland) session, run:");
            println!("  systemctl --user enable --now livewall.service");
            return Ok(());
        }
    }

    // Start it now (needs a running `systemd --user` session).
    match systemctl(&["start", "livewall.service"]) {
        Ok(_) => println!("started livewall.service"),
        Err(_) => println!("it will start on your next login (or run: systemctl --user start livewall.service)"),
    }
    Ok(())
}

fn autostart_disable() -> Result<()> {
    // Best-effort stop (needs a session bus); ignore failures when offline.
    let _ = systemctl(&["stop", "livewall.service"]);
    match systemctl(&["disable", "livewall.service"]) {
        Ok(_) => println!("disabled livewall.service"),
        Err(e) => println!("note: systemctl disable reported: {e}"),
    }
    let unit = unit_path()?;
    if unit.exists() {
        fs::remove_file(&unit).with_context(|| format!("removing {}", unit.display()))?;
        println!("removed {}", unit.display());
    } else {
        println!("no unit file at {}", unit.display());
    }
    let _ = systemctl(&["daemon-reload"]);
    Ok(())
}

fn autostart_status() -> Result<()> {
    let unit = unit_path()?;
    println!(
        "unit file: {} ({})",
        unit.display(),
        if unit.exists() { "installed" } else { "absent" }
    );
    match systemctl(&["is-enabled", "livewall.service"]) {
        Ok(s) => println!("enabled:   {s}"),
        Err(e) => println!("enabled:   {e}"),
    }
    match systemctl(&["is-active", "livewall.service"]) {
        Ok(s) => println!("active:    {s}"),
        Err(e) => println!("active:    {e}"),
    }
    Ok(())
}
