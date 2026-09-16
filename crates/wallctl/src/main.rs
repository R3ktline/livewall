use std::io::{BufReader, BufWriter};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use wall_core::{
    read_message, socket_path, write_message, HwDecPreference, Request, Response, ScaleMode,
};

#[derive(Parser, Debug)]
#[command(name = "wallctl", about = "Control the livewall wallpaper daemon")]
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
    },
    /// Clear wallpaper on an output
    Clear { output: String },
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
        .with_context(|| format!("connecting to {} (is walld running?)", path.display()))?;
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
    let json_status = matches!(cli.cmd, Cmd::Status { json: true });
    let req = match cli.cmd {
        Cmd::Ping => Request::Ping,
        Cmd::Set { output, path, mode } => {
            let path = path.canonicalize().unwrap_or(path);
            Request::Set { output, path, mode }
        }
        Cmd::Clear { output } => Request::Clear { output },
        Cmd::Pause => Request::Pause,
        Cmd::Resume => Request::Resume,
        Cmd::Status { .. } => Request::Status,
        Cmd::Shutdown => Request::Shutdown,
        Cmd::Hwdec { prefer } => Request::SetHwdec { hwdec: prefer },
    };

    let resp = roundtrip(req)?;

    match resp {
        Response::Ok { message } => {
            if let Some(m) = message {
                println!("{m}");
            }
        }
        Response::Pong => println!("pong"),
        Response::Error { message } => bail!("{message}"),
        Response::Status(info) => {
            if json_status {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                println!("paused:           {}", info.paused);
                println!("hwdec preference: {}", info.hwdec);
                println!("decode path:      {}", info.decode_path);
                println!("fps:              {:.1}", info.fps);
                println!("rss:              {:.1} MiB", info.rss_bytes as f64 / (1024.0 * 1024.0));
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
            }
        }
    }
    Ok(())
}
