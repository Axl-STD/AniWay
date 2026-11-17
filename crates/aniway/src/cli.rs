use aniway_proto::autostart;
use aniway_proto::ipc::{encode_request, Request, Response, StateData};
use aniway_proto::model::OverlayPatch;
use aniway_proto::paths;
use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser)]
#[command(name = "aniway", version, about = "Click-through media overlays for Hyprland")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the overlay daemon
    Daemon,
    /// Print state events as JSON lines until interrupted
    Watch,
    /// Add an overlay
    Add {
        path: String,
        #[arg(long)]
        id: Option<String>,
        #[command(flatten)]
        opts: Opts,
    },
    /// Change properties of an overlay
    Set {
        id: String,
        #[arg(long)]
        path: Option<String>,
        #[command(flatten)]
        opts: Opts,
    },
    /// Move an overlay (sugar for `set`)
    Move {
        id: String,
        #[arg(allow_hyphen_values = true)]
        pos: String,
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        anchor: Option<String>,
    },
    Remove {
        id: String,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Outputs {
        #[arg(long)]
        json: bool,
    },
    Show {
        id: Option<String>,
    },
    Hide {
        id: Option<String>,
        /// Hide everything for a while, e.g. 30s, 15m, 1h
        #[arg(long = "for")]
        duration: Option<String>,
    },
    Toggle {
        id: Option<String>,
    },
    Pause {
        id: Option<String>,
    },
    Resume {
        id: Option<String>,
    },
    TogglePause {
        id: Option<String>,
    },
    Scene {
        #[command(subcommand)]
        cmd: SceneCmd,
    },
    /// Start the daemon with the graphical session (systemd user unit)
    Autostart {
        #[command(subcommand)]
        cmd: AutostartCmd,
    },
    /// Read or change settings of config.toml
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Re-read config and scene files
    Reload,
    /// Stop the daemon
    Quit,
}

#[derive(Subcommand)]
enum SceneCmd {
    List,
    Load {
        name: String,
    },
    Save {
        name: String,
    },
    /// Create an empty scene and switch to it
    New {
        name: String,
    },
    Delete {
        name: String,
    },
    Rename {
        name: String,
        to: String,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print all settings, or the value of one
    Get { key: Option<String> },
    /// Change settings: key=value, where the value is JSON (strings may be bare), e.g. hide_for_classes='["mpv"]'
    Set {
        #[arg(required = true, value_name = "KEY=VALUE")]
        pairs: Vec<String>,
    },
}

#[derive(Subcommand)]
enum AutostartCmd {
    On,
    Off,
    Status,
}

#[derive(Args, Default)]
struct Opts {
    #[arg(long)]
    output: Option<String>,
    #[arg(long)]
    anchor: Option<String>,
    /// Offset from the anchor in logical pixels, X,Y
    #[arg(long, allow_hyphen_values = true)]
    pos: Option<String>,
    #[arg(long)]
    scale: Option<f64>,
    /// Size WxH; either side may be 0 to leave it free
    #[arg(long)]
    size: Option<String>,
    #[arg(long)]
    opacity: Option<f64>,
    #[arg(long)]
    speed: Option<f64>,
    #[arg(long)]
    layer: Option<String>,
    #[arg(long, conflicts_with = "looping")]
    no_loop: bool,
    #[arg(long = "loop")]
    looping: bool,
    #[arg(long)]
    volume: Option<u32>,
    /// Frame rate limit; 0 removes it
    #[arg(long)]
    max_fps: Option<u32>,
    /// auto, hardware or software
    #[arg(long)]
    decode: Option<String>,
    /// Allow (true) or forbid (false) a cached smaller copy of the media
    #[arg(long)]
    optimize: Option<bool>,
    /// Let the overlay go past the monitor edges by up to half its size (true) or keep it inside (false)
    #[arg(long)]
    overflow: Option<bool>,
}

pub fn run() -> Result<()> {
    for note in aniway_proto::legacy::migrate(sibling_daemon().as_deref()) {
        eprintln!("{note}");
    }
    let request = match Cli::parse().cmd {
        Cmd::Daemon => return exec_daemon(),
        Cmd::Add { path, id, opts } => {
            Request::Add { overlay: OverlayPatch { id, path: Some(absolute(&path)), ..opts.patch()? } }
        }
        Cmd::Set { id, path, opts } => {
            let patch = OverlayPatch { path: path.map(|p| absolute(&p)), ..opts.patch()? };
            Request::Set { id, patch, transient: false }
        }
        Cmd::Move { id, pos, output, anchor } => {
            let patch = Opts { pos: Some(pos), output, anchor, ..Opts::default() }.patch()?;
            Request::Set { id, patch, transient: false }
        }
        Cmd::Remove { id } => Request::Remove { id },
        Cmd::List { json } => return list(json),
        Cmd::Outputs { json } => return outputs(json),
        Cmd::Show { id } => Request::Show { id },
        Cmd::Hide { id, duration } => Request::Hide { id, for_secs: duration.map(|d| parse_duration(&d)).transpose()? },
        Cmd::Toggle { id } => Request::Toggle { id },
        Cmd::Pause { id } => Request::Pause { id },
        Cmd::Resume { id } => Request::Resume { id },
        Cmd::TogglePause { id } => Request::TogglePause { id },
        Cmd::Scene { cmd: SceneCmd::List } => return scene_list(),
        Cmd::Scene { cmd: SceneCmd::Load { name } } => Request::SceneLoad { name },
        Cmd::Scene { cmd: SceneCmd::Save { name } } => Request::SceneSave { name },
        Cmd::Scene { cmd: SceneCmd::New { name } } => Request::SceneNew { name },
        Cmd::Scene { cmd: SceneCmd::Delete { name } } => Request::SceneDelete { name },
        Cmd::Scene { cmd: SceneCmd::Rename { name, to } } => Request::SceneRename { name, to },
        Cmd::Autostart { cmd } => return autostart(&cmd),
        Cmd::Config { cmd: ConfigCmd::Get { key } } => return config_get(key.as_deref()),
        Cmd::Config { cmd: ConfigCmd::Set { pairs } } => Request::ConfigSet { patch: config_patch(&pairs)? },
        Cmd::Watch => return watch(),
        Cmd::Reload => Request::Reload,
        Cmd::Quit => Request::Quit,
    };
    if let Value::String(id) = send(&request)? {
        println!("{id}");
    }
    Ok(())
}

impl Opts {
    fn patch(&self) -> Result<OverlayPatch> {
        let pos = self.pos.as_deref().map(|p| pair(p, ',')).transpose()?;
        let size = self.size.as_deref().map(|s| pair(s, 'x')).transpose()?;
        Ok(OverlayPatch {
            output: self.output.clone(),
            anchor: self.anchor.as_deref().map(parse_enum).transpose()?,
            x: pos.map(|p| p.0),
            y: pos.map(|p| p.1),
            scale: self.scale,
            width: size.map(|s| s.0.max(0) as u32),
            height: size.map(|s| s.1.max(0) as u32),
            opacity: self.opacity,
            speed: self.speed,
            layer: self.layer.as_deref().map(parse_enum).transpose()?,
            looping: if self.no_loop { Some(false) } else { self.looping.then_some(true) },
            volume: self.volume,
            max_fps: self.max_fps,
            decode: self.decode.as_deref().map(parse_enum).transpose()?,
            optimize: self.optimize,
            overflow: self.overflow,
            ..OverlayPatch::default()
        })
    }
}

/// Absolute path of the `aniwayd` installed next to this executable.
fn sibling_daemon() -> Option<PathBuf> {
    std::env::current_exe().ok()?.with_file_name("aniwayd").canonicalize().ok()
}

/// Replaces this process with `aniwayd`, looked up next to this executable and then in PATH.
fn exec_daemon() -> Result<()> {
    let err = Command::new(sibling_daemon().unwrap_or_else(|| "aniwayd".into())).exec();
    bail!("cannot execute aniwayd: {err}")
}

fn autostart(cmd: &AutostartCmd) -> Result<()> {
    match cmd {
        AutostartCmd::On => {
            let exec = sibling_daemon().ok_or_else(|| anyhow!("aniwayd not found next to this executable"))?;
            autostart::enable(&exec).context("writing the systemd user unit")?;
            println!("autostart enabled: {}", exec.display());
            println!("takes effect at the next login");
        }
        AutostartCmd::Off => {
            autostart::disable().context("removing the systemd user unit")?;
            println!("autostart disabled");
        }
        AutostartCmd::Status => {
            let st = autostart::status();
            println!("autostart: {}", if st.enabled { "enabled" } else { "disabled" });
            if let Some(exec) = st.exec {
                let note = if st.exec_exists { "" } else { " (missing: run `aniway autostart on` again)" };
                println!("ExecStart: {}{note}", exec.display());
            }
            let active = Command::new("systemctl").args(["--user", "is-active", "graphical-session.target"]).output();
            if !active.is_ok_and(|o| o.status.success()) {
                println!("note: this session does not start graphical-session.target; add `exec-once = aniwayd` to your compositor config instead");
            }
        }
    }
    Ok(())
}

fn pair(text: &str, sep: char) -> Result<(i32, i32)> {
    let (a, b) =
        text.split_once(sep).ok_or_else(|| anyhow!("expected two numbers separated by '{sep}', got '{text}'"))?;
    Ok((
        a.trim().parse().with_context(|| format!("invalid number '{a}'"))?,
        b.trim().parse().with_context(|| format!("invalid number '{b}'"))?,
    ))
}

fn parse_enum<T: DeserializeOwned>(text: &str) -> Result<T> {
    serde_json::from_value(Value::String(text.to_string())).map_err(|_| anyhow!("invalid value '{text}'"))
}

/// Parses `30s`, `15m`, `1h` or a bare number of seconds.
fn parse_duration(text: &str) -> Result<u64> {
    let (digits, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len()));
    let n: u64 = digits.parse().map_err(|_| anyhow!("invalid duration '{text}'"))?;
    match unit {
        "" | "s" => Ok(n),
        "m" => Ok(n * 60),
        "h" => Ok(n * 3600),
        _ => bail!("invalid duration '{text}' (use 30s, 15m or 1h)"),
    }
}

/// The daemon runs elsewhere, so relative file paths are made absolute here.
fn absolute(path: &str) -> String {
    if paths::is_url(path) || path.starts_with('~') {
        return path.into();
    }
    std::fs::canonicalize(path).map_or_else(|_| path.into(), |p| p.to_string_lossy().into_owned())
}

fn connect() -> Result<UnixStream> {
    UnixStream::connect(paths::socket_path()).map_err(|e| match e.kind() {
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => {
            anyhow!("daemon not running (start it with `aniway daemon`)")
        }
        _ => anyhow!(e),
    })
}

fn check(line: &str) -> Result<Value> {
    let response: Response = serde_json::from_str(line).context("invalid daemon response")?;
    if !response.ok {
        bail!("{}", response.error.unwrap_or_default());
    }
    Ok(response.data.unwrap_or(Value::Null))
}

fn watch() -> Result<()> {
    let mut stream = connect()?;
    writeln!(stream, "{}", encode_request(&Request::Subscribe))?;
    let mut lines = BufReader::new(stream).lines();
    check(&lines.next().context("daemon closed the connection")??)?;
    for line in lines {
        println!("{}", line?);
    }
    Ok(())
}

fn send(request: &Request) -> Result<Value> {
    let mut stream = connect()?;
    writeln!(stream, "{}", encode_request(request))?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    check(&line)
}

fn state() -> Result<StateData> {
    Ok(serde_json::from_value(send(&Request::State)?)?)
}

fn list(json: bool) -> Result<()> {
    let state = state()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&state.overlays)?);
        return Ok(());
    }
    println!("{:<14} {:<10} {:<12} {:<18} {:<10} {:<11} STATE", "ID", "OUTPUT", "ANCHOR", "RECT", "MEDIA", "HWDEC");
    for o in &state.overlays {
        let rect = o.rect.map_or("-".into(), |r| format!("{}x{}+{}+{}", r.w, r.h, r.x, r.y));
        let media = o.media.map_or("-".into(), |m| format!("{}x{}", m.w, m.h));
        let anchor = serde_json::to_value(o.config.anchor)?;
        let status = match (&o.error, o.shown, o.paused) {
            (Some(e), ..) => format!("error: {e}"),
            (None, true, false) => "playing".into(),
            (None, true, true) => "paused".into(),
            (None, false, _) => "hidden".into(),
        };
        let proxy = serde_json::to_value(o.proxy)?;
        let status = match (proxy.as_str(), &o.proxy_error) {
            (Some("off") | None, _) => status,
            (Some(p), Some(e)) => format!("{status} (copy {p}: {e})"),
            (Some(p), None) => format!("{status} (copy: {p})"),
        };
        println!(
            "{:<14} {:<10} {:<12} {:<18} {:<10} {:<11} {status}",
            o.config.id,
            o.config.output.as_deref().unwrap_or("(first)"),
            anchor.as_str().unwrap_or_default(),
            rect,
            media,
            o.hwdec.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}

fn outputs(json: bool) -> Result<()> {
    let outputs = state()?.outputs;
    if json {
        println!("{}", serde_json::to_string_pretty(&outputs)?);
        return Ok(());
    }
    println!("{:<14} {:<18} {:<6} DESCRIPTION", "NAME", "GEOMETRY", "SCALE");
    for o in outputs {
        println!(
            "{:<14} {:<18} {:<6} {}",
            o.name,
            format!("{}x{}+{}+{}", o.rect.w, o.rect.h, o.rect.x, o.rect.y),
            o.scale,
            o.description
        );
    }
    Ok(())
}

/// `key=value` pairs to a JSON object; a value that is not valid JSON is taken as a string.
fn config_patch(pairs: &[String]) -> Result<Value> {
    let mut patch = serde_json::Map::new();
    for pair in pairs {
        let (key, value) = pair.split_once('=').ok_or_else(|| anyhow!("expected key=value, got '{pair}'"))?;
        let value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.into()));
        patch.insert(key.trim().into(), value);
    }
    Ok(patch.into())
}

fn config_get(key: Option<&str>) -> Result<()> {
    let config = serde_json::to_value(state()?.config)?;
    match key {
        None => print!("{}", aniway_proto::model::to_toml(&config)),
        Some(key) => match &config[key] {
            Value::Null => bail!("unknown setting '{key}'"),
            Value::String(s) => println!("{s}"),
            other => println!("{other}"),
        },
    }
    Ok(())
}

fn scene_list() -> Result<()> {
    let data = send(&Request::SceneList)?;
    let active = data["active"].as_str().unwrap_or_default();
    for name in data["scenes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        println!("{} {name}", if name == active { "*" } else { " " });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30s").unwrap(), 30);
        assert_eq!(parse_duration("15m").unwrap(), 900);
        assert_eq!(parse_duration("1h").unwrap(), 3600);
        assert_eq!(parse_duration("45").unwrap(), 45);
        assert!(parse_duration("1d").is_err());
        assert!(parse_duration("m").is_err());
    }

    #[test]
    fn config_pairs_become_a_patch() {
        let patch = config_patch(&["hwdec=vaapi".into(), "optimize=false".into(), "hide_for_classes=[\"mpv\"]".into()]);
        assert_eq!(
            patch.unwrap(),
            serde_json::json!({"hwdec": "vaapi", "optimize": false, "hide_for_classes": ["mpv"]})
        );
        assert!(config_patch(&["nokey".into()]).is_err());
    }

    #[test]
    fn pairs_and_enums() {
        assert_eq!(pair("-20,-20", ',').unwrap(), (-20, -20));
        assert_eq!(pair("400x0", 'x').unwrap(), (400, 0));
        assert!(pair("4", ',').is_err());
        assert_eq!(
            parse_enum::<aniway_proto::model::Anchor>("bottom-right").unwrap(),
            aniway_proto::model::Anchor::BottomRight
        );
        assert!(parse_enum::<aniway_proto::model::Anchor>("middle").is_err());
    }

    #[test]
    fn options_become_a_patch() {
        let opts = Opts {
            pos: Some("5,-6".into()),
            size: Some("100x0".into()),
            no_loop: true,
            decode: Some("software".into()),
            max_fps: Some(30),
            ..Opts::default()
        };
        let p = opts.patch().unwrap();
        assert_eq!((p.decode, p.max_fps), (Some(aniway_proto::model::Decode::Software), Some(30)));
        assert_eq!((p.x, p.y, p.width, p.height, p.looping), (Some(5), Some(-6), Some(100), Some(0), Some(false)));
    }
}
