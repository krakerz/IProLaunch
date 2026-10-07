//! Headless Sway "virtual desktop" for Sunshine streams.
//!
//! `service` is meant to replace Sunshine's own systemd `ExecStart`: it starts
//! a headless Sway, then runs Sunshine inside that Wayland display so Sunshine
//! captures only it, and games Sunshine launches inherit it (plus the stream's
//! audio sink). While running it keeps Sunshine's virtual input devices
//! enabled in the headless Sway and disabled on the host desktop. `prep` is
//! the per-app Sunshine `prep-cmd`: resizes the virtual output to the client
//! and creates the stream's audio sink.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::config::{Config, Profile, StreamMode, Sunshine};

/// Sunshine's virtual devices: inputtino/libvirtualhid (vendor 0x1209) and
/// its passthrough devices (vendor 0xbeef). Sway identifiers are `vendor:product:name`.
const SUNSHINE_VENDOR_PREFIXES: [&str; 2] = ["4617:", "48879:"];
const HOST_SWAYSOCK_ENV: &str = "IPROLAUNCH_HOST_SWAYSOCK";
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", current_uid())))
}

fn current_uid() -> u32 {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(1000)
}

fn state_dir() -> PathBuf {
    runtime_dir().join("iprolaunch-sunshine")
}

pub fn swaysock(cfg: &Sunshine) -> PathBuf {
    runtime_dir().join(&cfg.socket_name)
}

/// Whether a device belongs to Sunshine. `match_names` (config `input_match`)
/// replaces the name-based auto-detection when non-empty.
fn is_sunshine_device(identifier: &str, name: &str, match_names: &[String]) -> bool {
    if !match_names.is_empty() {
        return match_names.iter().any(|m| m == name);
    }
    let lower = name.to_ascii_lowercase();
    SUNSHINE_VENDOR_PREFIXES
        .iter()
        .any(|p| identifier.starts_with(p))
        || lower.starts_with("libvirtualhid")
        || lower.contains("passthrough")
}

pub fn sway_config(cfg: &Sunshine) -> String {
    let mut out = format!(
        "output * mode {}x{}@{}Hz scale {}\n\
         swaybg_command /usr/bin/true\n\
         default_border none\n\
         input * events disabled\n",
        cfg.width, cfg.height, cfg.refresh, cfg.scale
    );
    if cfg.hide_cursor {
        out.push_str("seat * hide_cursor 1\n");
    }
    out
}

fn wayland_sockets() -> HashSet<String> {
    fs::read_dir(runtime_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with("wayland-") && !n.ends_with(".lock"))
                .collect()
        })
        .unwrap_or_default()
}

fn swaymsg_json(sock: &Path, kind: &str) -> Result<Value> {
    let out = Command::new("swaymsg")
        .arg("-s")
        .arg(sock)
        .args(["-r", "-t", kind])
        .output()
        .context("running swaymsg")?;
    if !out.status.success() {
        bail!(
            "swaymsg -t {kind} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).context("parsing swaymsg output")
}

fn swaymsg(sock: &Path, command: &str) -> Result<()> {
    let out = Command::new("swaymsg")
        .arg("-s")
        .arg(sock)
        .arg(command)
        .output()
        .context("running swaymsg")?;
    if !out.status.success() {
        bail!(
            "swaymsg `{command}` failed: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        );
    }
    Ok(())
}

/// `(identifier, name, type)` for each input a Sway instance reports.
fn sway_inputs(sock: &Path) -> Result<Vec<(String, String, String)>> {
    let json = swaymsg_json(sock, "get_inputs")?;
    Ok(json
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            let field = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            (field("identifier"), field("name"), field("type"))
        })
        .collect())
}

fn spawn_sway(cfg: &Sunshine, config_path: &Path) -> Result<Child> {
    Command::new("sway")
        .arg("--config")
        .arg(config_path)
        .env("WLR_BACKENDS", "headless,libinput")
        .env("LIBSEAT_BACKEND", "noop")
        .env("SWAYSOCK", swaysock(cfg))
        .env("XDG_SESSION_TYPE", "wayland")
        .env("XDG_CURRENT_DESKTOP", "sway")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .spawn()
        .context("starting headless sway (is `sway` installed?)")
}

/// Waits for the headless Sway to create its Wayland socket and IPC socket.
fn wait_for_display(cfg: &Sunshine, before: &HashSet<String>, sway: &mut Child) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(status) = sway.try_wait()? {
            bail!("headless sway exited during startup ({status})");
        }
        let new = wayland_sockets().into_iter().find(|s| !before.contains(s));
        if let Some(display) = new
            && swaysock(cfg).exists()
        {
            return Ok(display);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    bail!("headless sway didn't create its sockets within 15s")
}

/// Runs the headless virtual desktop and Sunshine inside it until either exits.
pub fn run_service(cfg: &Config, sunshine_bin: &str) -> Result<()> {
    let s = &cfg.sunshine;
    let dir = state_dir();
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let config_path = dir.join("sway.conf");
    fs::write(&config_path, sway_config(s))
        .with_context(|| format!("writing {}", config_path.display()))?;
    let _ = fs::remove_file(swaysock(s));

    let host_swaysock = std::env::var_os("SWAYSOCK").map(PathBuf::from);
    let before = wayland_sockets();
    let mut sway = spawn_sway(s, &config_path)?;
    let display = match wait_for_display(s, &before, &mut sway) {
        Ok(d) => d,
        Err(err) => {
            let _ = sway.kill();
            return Err(err);
        }
    };
    eprintln!("iprolaunch: virtual desktop up on {display}");

    let mut sunshine = Command::new(sunshine_bin);
    sunshine
        .env("WAYLAND_DISPLAY", &display)
        .env("SWAYSOCK", swaysock(s))
        .env("XDG_CURRENT_DESKTOP", "sway")
        .env("PULSE_SINK", &s.audio_sink)
        .env("PULSE_PROP", format!("{STREAM_PROP}=game"))
        .env("PIPEWIRE_PROPS", format!("{{ {STREAM_PROP} = game }}"))
        .env_remove("DISPLAY");
    if let Some(host) = &host_swaysock {
        sunshine.env(HOST_SWAYSOCK_ENV, host);
    }
    let mut sunshine = match sunshine.spawn() {
        Ok(c) => c,
        Err(err) => {
            let _ = sway.kill();
            return Err(err).with_context(|| format!("starting {sunshine_bin}"));
        }
    };

    let host = HostDesktop::detect(host_swaysock);
    if s.input_isolation && matches!(host, HostDesktop::Unknown) {
        eprintln!(
            "iprolaunch: couldn't tell which desktop this is (Hyprland/Sway/KDE) — \
             streamed input may also reach the host desktop"
        );
    }
    let mut watcher = InputWatcher::default();
    let (audio_events, mut subscriber) = subscribe_audio_events();
    let result = loop {
        if let Some(status) = sunshine.try_wait()? {
            let _ = sway.kill();
            let _ = sway.wait();
            if status.success() {
                break Ok(());
            }
            break Err(anyhow::anyhow!("sunshine exited with {status}"));
        }
        if let Some(status) = sway.try_wait()? {
            let _ = sunshine.kill();
            let _ = sunshine.wait();
            break Err(anyhow::anyhow!("headless sway exited ({status})"));
        }
        watcher.enable_in_virtual_desktop(s);
        if s.input_isolation {
            watcher.isolate_from_host(&host, &s.input_match);
        }
        if let Err(err) = route_stream_audio(sunshine.id(), &s.audio_sink) {
            eprintln!("iprolaunch: audio routing: {err:#}");
        }
        // Wake early when an audio stream appears, so a game that reopens its
        // output (e.g. on pause) is moved before it's audible on the host.
        let _ = audio_events.recv_timeout(WATCH_INTERVAL);
        while audio_events.try_recv().is_ok() {}
    };
    if let Some(child) = subscriber.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

/// Pulse/PipeWire stream property marking audio from games Sunshine launched.
const STREAM_PROP: &str = "iprolaunch.stream";

/// Spawns `pactl subscribe` and forwards a tick for each new/changed playback
/// stream. Without pactl the channel just never fires and polling still works.
fn subscribe_audio_events() -> (std::sync::mpsc::Receiver<()>, Option<Child>) {
    use std::io::{BufRead, BufReader};
    let (tx, rx) = std::sync::mpsc::channel();
    let child = Command::new("pactl")
        .arg("subscribe")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return (rx, None);
    };
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.contains("sink-input")
                    && (line.contains("'new'") || line.contains("'change'"))
                    && tx.send(()).is_err()
                {
                    break;
                }
            }
        });
    }
    (rx, Some(child))
}

fn parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 4, after the `(comm)` which may itself contain spaces/parens.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn descends_from(mut pid: u32, ancestor: u32) -> bool {
    for _ in 0..64 {
        if pid == ancestor {
            return true;
        }
        match parent_pid(pid) {
            Some(p) if p > 1 => pid = p,
            _ => return false,
        }
    }
    false
}

/// A playback stream as `pactl -f json list sink-inputs` reports it.
struct SinkInput {
    index: u64,
    sink: u64,
    pid: Option<u32>,
    marked: bool,
}

fn parse_sink_inputs(json: &Value) -> Vec<SinkInput> {
    json.as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| {
            let props = i.get("properties");
            let prop = |k: &str| props.and_then(|p| p.get(k)).and_then(Value::as_str);
            Some(SinkInput {
                index: i.get("index")?.as_u64()?,
                sink: i.get("sink")?.as_u64()?,
                pid: prop("application.process.id").and_then(|p| p.parse().ok()),
                marked: prop(STREAM_PROP) == Some("game"),
            })
        })
        .collect()
}

fn pactl_json(args: &[&str]) -> Result<Value> {
    let out = Command::new("pactl")
        .args(["-f", "json"])
        .args(args)
        .output()
        .context("running pactl")?;
    if !out.status.success() {
        bail!(
            "pactl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    serde_json::from_slice(&out.stdout).context("parsing pactl output")
}

/// Moves every playback stream from a game Sunshine launched onto the stream
/// sink. Wine/Proton picks its output device itself and doesn't reliably
/// honor `PULSE_SINK`, especially when it reopens audio (pause, alt-tab).
fn route_stream_audio(sunshine_pid: u32, sink_name: &str) -> Result<()> {
    let sinks = pactl_json(&["list", "sinks"])?;
    let Some(sink_index) = sinks
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s.get("name").and_then(Value::as_str) == Some(sink_name))
        .and_then(|s| s.get("index").and_then(Value::as_u64))
    else {
        return Ok(()); // no stream running, nothing to route
    };
    for input in parse_sink_inputs(&pactl_json(&["list", "sink-inputs"])?) {
        let ours = input.marked || input.pid.is_some_and(|p| descends_from(p, sunshine_pid));
        if ours && input.sink != sink_index {
            Command::new("pactl")
                .args(["move-sink-input", &input.index.to_string(), sink_name])
                .status()
                .context("running pactl move-sink-input")?;
        }
    }
    Ok(())
}

pub enum HostDesktop {
    Hyprland,
    Sway(PathBuf),
    Kde,
    Unknown,
}

impl HostDesktop {
    fn detect(host_swaysock: Option<PathBuf>) -> Self {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            return HostDesktop::Hyprland;
        }
        if let Some(sock) = host_swaysock.filter(|p| p.exists()) {
            return HostDesktop::Sway(sock);
        }
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        if std::env::var_os("KDE_SESSION_VERSION").is_some() || desktop.contains("KDE") {
            return HostDesktop::Kde;
        }
        HostDesktop::Unknown
    }
}

/// Remembers what's already been switched so each device is touched once
/// while it exists, not every poll.
#[derive(Default)]
struct InputWatcher {
    enabled: HashSet<String>,
    host_disabled: HashSet<String>,
}

impl InputWatcher {
    fn enable_in_virtual_desktop(&mut self, cfg: &Sunshine) {
        let sock = swaysock(cfg);
        let Ok(inputs) = sway_inputs(&sock) else {
            return;
        };
        let present: HashSet<String> = inputs.iter().map(|(id, _, _)| id.clone()).collect();
        self.enabled.retain(|id| present.contains(id));
        for (id, name, kind) in inputs {
            if self.enabled.contains(&id) || !is_sunshine_device(&id, &name, &cfg.input_match) {
                continue;
            }
            let quoted = format!("\"{}\"", id.replace('"', "\\\""));
            if swaymsg(&sock, &format!("input {quoted} events enabled")).is_ok() {
                if kind == "pointer" {
                    let _ = swaymsg(&sock, &format!("input {quoted} accel_profile flat"));
                }
                self.enabled.insert(id);
            }
        }
    }

    fn isolate_from_host(&mut self, host: &HostDesktop, match_names: &[String]) {
        let result = match host {
            HostDesktop::Hyprland => self.isolate_hyprland(match_names),
            HostDesktop::Sway(sock) => self.isolate_sway(sock, match_names),
            HostDesktop::Kde => self.isolate_kde(match_names),
            HostDesktop::Unknown => Ok(()),
        };
        if let Err(err) = result {
            eprintln!("iprolaunch: host input isolation: {err:#}");
        }
    }

    /// The Hyprland rule outlives the device, so each name only needs it once
    /// per service run.
    fn isolate_hyprland(&mut self, match_names: &[String]) -> Result<()> {
        let out = Command::new("hyprctl")
            .args(["devices", "-j"])
            .output()
            .context("running hyprctl")?;
        let json: Value = serde_json::from_slice(&out.stdout).context("parsing hyprctl devices")?;
        let names: Vec<String> = ["mice", "keyboards", "tablets", "touch"]
            .iter()
            .filter_map(|k| json.get(*k).and_then(Value::as_array))
            .flatten()
            .filter_map(|d| d.get("name").and_then(Value::as_str))
            .filter(|n| is_sunshine_device("", n, match_names))
            .map(str::to_string)
            .collect();
        for name in names {
            if self.host_disabled.contains(&name) {
                continue;
            }
            hyprland_disable(&name)?;
            self.host_disabled.insert(name);
        }
        Ok(())
    }

    fn isolate_sway(&mut self, sock: &Path, match_names: &[String]) -> Result<()> {
        let inputs = sway_inputs(sock)?;
        let present: HashSet<String> = inputs.iter().map(|(id, _, _)| id.clone()).collect();
        self.host_disabled.retain(|id| present.contains(id));
        for (id, name, _) in inputs {
            if self.host_disabled.contains(&id) || !is_sunshine_device(&id, &name, match_names) {
                continue;
            }
            let quoted = format!("\"{}\"", id.replace('"', "\\\""));
            swaymsg(sock, &format!("input {quoted} events disabled"))?;
            self.host_disabled.insert(id);
        }
        Ok(())
    }

    fn isolate_kde(&mut self, match_names: &[String]) -> Result<()> {
        let devices = busctl_get(
            "/org/kde/KWin/InputDevice",
            "org.kde.KWin.InputDeviceManager",
            "devicesSysNames",
        )?;
        let sys_names = parse_busctl_strings(&devices);
        let present: HashSet<String> = sys_names.iter().cloned().collect();
        self.host_disabled.retain(|d| present.contains(d));
        for sys in sys_names {
            if self.host_disabled.contains(&sys) {
                continue;
            }
            let path = format!("/org/kde/KWin/InputDevice/{sys}");
            let name = busctl_get(&path, "org.kde.KWin.InputDevice", "name")
                .map(|raw| {
                    parse_busctl_strings(&raw)
                        .into_iter()
                        .next()
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            if !is_sunshine_device("", &name, match_names) {
                continue;
            }
            let status = Command::new("busctl")
                .args(["--user", "set-property", "org.kde.KWin", &path])
                .args(["org.kde.KWin.InputDevice", "enabled", "b", "false"])
                .status()
                .context("running busctl")?;
            if status.success() {
                self.host_disabled.insert(sys);
            }
        }
        Ok(())
    }
}

/// Hyprland 0.56+ is Lua-configured (`keyword` is rejected there); older
/// builds only have `keyword`.
fn hyprland_disable(name: &str) -> Result<()> {
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    let lua = format!("hl.device({{ name = \"{escaped}\", enabled = false }})");
    if hyprctl_ok(&["eval", &lua]) {
        return Ok(());
    }
    if hyprctl_ok(&["keyword", &format!("device[{name}]:enabled"), "false"]) {
        return Ok(());
    }
    bail!("hyprctl couldn't disable `{name}`")
}

/// hyprctl exits 0 even on a rejected command, so judge by its reply text.
fn hyprctl_ok(args: &[&str]) -> bool {
    Command::new("hyprctl").args(args).output().is_ok_and(|o| {
        let text = String::from_utf8_lossy(&o.stdout).to_ascii_lowercase();
        o.status.success()
            && !["error", "can't", "invalid", "unknown"]
                .iter()
                .any(|bad| text.contains(bad))
    })
}

fn busctl_get(path: &str, interface: &str, property: &str) -> Result<String> {
    let out = Command::new("busctl")
        .args([
            "--user",
            "get-property",
            "org.kde.KWin",
            path,
            interface,
            property,
        ])
        .output()
        .context("running busctl")?;
    if !out.status.success() {
        bail!(
            "busctl get-property {property} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Pulls the quoted strings out of busctl's `as 2 "a" "b"` / `s "a"` output.
fn parse_busctl_strings(raw: &str) -> Vec<String> {
    raw.split('"')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .collect()
}

fn client_mode_from_env() -> Option<(u32, u32, u32)> {
    let var = |k: &str| {
        std::env::var(k)
            .ok()?
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|v| *v > 0)
    };
    Some((
        var("SUNSHINE_CLIENT_WIDTH")?,
        var("SUNSHINE_CLIENT_HEIGHT")?,
        var("SUNSHINE_CLIENT_FPS")?,
    ))
}

/// Channel count for the stream's sink: the configured layout, else the
/// client's (`SUNSHINE_CLIENT_AUDIO_CONFIGURATION`, e.g. `2.0`/`5.1`/`7.1`).
fn channel_count(cfg: &Sunshine, client_audio: Option<&str>) -> u32 {
    cfg.audio_channels
        .count()
        .unwrap_or_else(|| match client_audio {
            Some(c) if c.starts_with("7.1") => 8,
            Some(c) if c.starts_with("5.1") => 6,
            _ => 2,
        })
}

fn set_output_mode(cfg: &Sunshine, mode: StreamMode) -> Result<()> {
    swaymsg(
        &swaysock(cfg),
        &format!(
            "output * mode {}x{}@{}Hz scale {}",
            mode.width, mode.height, mode.refresh, cfg.scale
        ),
    )
}

fn sink_module_file() -> PathBuf {
    state_dir().join("sink-module-id")
}

fn unload_sink() {
    if let Ok(id) = fs::read_to_string(sink_module_file()) {
        let _ = Command::new("pactl")
            .args(["unload-module", id.trim()])
            .status();
        let _ = fs::remove_file(sink_module_file());
    }
}

fn load_sink(cfg: &Sunshine, channels: u32) -> Result<()> {
    unload_sink();
    let out = Command::new("pactl")
        .args([
            "load-module",
            "module-null-sink",
            &format!("sink_name={}", cfg.audio_sink),
            &format!("channels={channels}"),
            &format!("sink_properties=device.description={}", cfg.audio_sink),
        ])
        .output()
        .context("running pactl (is pipewire-pulse running?)")?;
    if !out.status.success() {
        bail!(
            "pactl load-module failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    fs::create_dir_all(state_dir())?;
    fs::write(
        sink_module_file(),
        String::from_utf8_lossy(&out.stdout).trim(),
    )?;
    Ok(())
}

/// Sunshine `prep-cmd`. Never fails the stream: problems are reported on
/// stderr (Sunshine's log) and the game still launches.
pub fn prep(cfg: &Config, action: PrepAction, slug: Option<&str>) {
    let profile = slug.and_then(|s| Profile::load(s).ok());
    let s = &cfg.sunshine;
    let report = |what: &str, r: Result<()>| {
        if let Err(err) = r {
            eprintln!("iprolaunch: {what}: {err:#}");
        }
    };
    match action {
        PrepAction::Do => {
            let mode = cfg.effective_stream_mode(profile.as_ref(), client_mode_from_env());
            report("resizing the virtual desktop", set_output_mode(s, mode));
            let client_audio = std::env::var("SUNSHINE_CLIENT_AUDIO_CONFIGURATION").ok();
            let channels = channel_count(s, client_audio.as_deref());
            report("creating the audio sink", load_sink(s, channels));
        }
        PrepAction::Undo => {
            let mode = cfg.effective_stream_mode(None, None);
            report("resetting the virtual desktop", set_output_mode(s, mode));
            unload_sink();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum PrepAction {
    Do,
    Undo,
}

const SUNSHINE_UNITS: [&str; 2] = [
    "app-dev.lizardbyte.app.Sunshine.service",
    "sunshine.service",
];

fn find_sunshine_unit() -> Option<&'static str> {
    SUNSHINE_UNITS.into_iter().find(|unit| {
        Command::new("systemctl")
            .args(["--user", "cat", unit])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

pub fn override_conf(bin: &Path, sunshine_bin: &str) -> String {
    format!(
        "# Written by iprolaunch: runs Sunshine inside its headless virtual desktop.\n\
         [Service]\n\
         ExecStart=\n\
         ExecStart=\"{}\" sunshine service --sunshine \"{sunshine_bin}\"\n",
        bin.display()
    )
}

fn override_path(unit: &str) -> Result<PathBuf> {
    let home = directories::UserDirs::new()
        .context("no home directory")?
        .home_dir()
        .to_path_buf();
    Ok(home
        .join(".config/systemd/user")
        .join(format!("{unit}.d"))
        .join("override.conf"))
}

/// One-line state of the Sunshine service setup, for the TUI.
pub fn service_status() -> String {
    let Some(unit) = find_sunshine_unit() else {
        return "Sunshine service not found".to_string();
    };
    let ours = override_path(unit)
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .is_some_and(|s| s.contains("Written by iprolaunch"));
    let active = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", unit])
        .status()
        .is_ok_and(|s| s.success());
    format!(
        "{} ({})",
        if ours { "installed" } else { "not installed" },
        if active { "running" } else { "stopped" }
    )
}

/// Undoes `install_service`: puts back the override it backed up, or removes
/// its own override when there was none before.
pub fn restore_service() -> Result<()> {
    let unit = find_sunshine_unit().context("no Sunshine systemd user service found")?;
    let path = override_path(unit)?;
    let backup = path.with_file_name("override.conf.bak");
    let ours = fs::read_to_string(&path).is_ok_and(|s| s.contains("Written by iprolaunch"));
    let mut restored = false;
    if backup.exists() {
        fs::rename(&backup, &path).with_context(|| format!("restoring {}", backup.display()))?;
        println!("Restored {} from its backup.", path.display());
        restored = true;
    } else if ours {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        if let Some(dir) = path.parent() {
            let _ = fs::remove_dir(dir); // only succeeds when empty
        }
        println!(
            "Removed {} — Sunshine's service is back to its default.",
            path.display()
        );
        restored = true;
    }
    let conf = sunshine_conf_path()?;
    let conf_backup = conf_backup_path(&conf);
    if conf_backup.exists() {
        fs::rename(&conf_backup, &conf)
            .with_context(|| format!("restoring {}", conf_backup.display()))?;
        println!("Restored {} from its backup.", conf.display());
        restored = true;
    }
    if !restored {
        bail!("nothing to restore — the Sunshine service isn't set up by iprolaunch");
    }
    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .context("running systemctl --user daemon-reload")?;
    println!("Restart Sunshine for this to take effect.");
    Ok(())
}

pub fn restart_service() -> Result<()> {
    let unit = find_sunshine_unit().context("no Sunshine systemd user service found")?;
    println!("Restarting {unit}...");
    let status = Command::new("systemctl")
        .args(["--user", "restart", unit])
        .status()
        .context("running systemctl")?;
    if !status.success() {
        bail!("systemctl --user restart {unit} failed ({status})");
    }
    println!("Restarted.");
    Ok(())
}

/// Points Sunshine's systemd user service at `iprolaunch sunshine service`,
/// backing up any existing override first (see `restore_service`).
pub fn install_service(cfg: &Config) -> Result<()> {
    let unit = find_sunshine_unit()
        .context("no Sunshine systemd user service found (looked for app-dev.lizardbyte.app.Sunshine.service and sunshine.service)")?;
    let sunshine_bin = which("sunshine").context("`sunshine` isn't on $PATH")?;
    let bin = std::env::current_exe()?.canonicalize()?;
    let path = override_path(unit)?;
    let dropin = path.parent().context("override path has no parent")?;
    fs::create_dir_all(dropin)?;
    if let Ok(existing) = fs::read_to_string(&path)
        && !existing.contains("Written by iprolaunch")
    {
        let backup = dropin.join("override.conf.bak");
        fs::write(&backup, existing)?;
        println!("Backed up the existing override to {}", backup.display());
    }
    fs::write(&path, override_conf(&bin, &sunshine_bin))?;
    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .context("running systemctl --user daemon-reload")?;

    println!("Installed {}", path.display());

    let conf = sunshine_conf_path()?;
    let text = fs::read_to_string(&conf).unwrap_or_default();
    let conf_backup = conf_backup_path(&conf);
    if !conf_backup.exists() {
        fs::write(&conf_backup, &text).with_context(|| format!("backing up {}", conf.display()))?;
    }
    if let Some(dir) = conf.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(
        &conf,
        set_conf_key(&text, "audio_sink", &cfg.sunshine.audio_sink),
    )
    .with_context(|| format!("writing {}", conf.display()))?;
    println!(
        "Set audio_sink = {} in {}",
        cfg.sunshine.audio_sink,
        conf.display()
    );

    println!();
    match cfg.sunshine.auth_token.as_deref() {
        Some(token) => match crate::sunshine::refresh_prep_cmds(&cfg.sunshine, token) {
            Ok(0) => println!("Games already in Sunshine are up to date."),
            Ok(n) => println!("Updated {n} game(s) already in Sunshine to the new prep commands."),
            Err(err) => println!("Couldn't update games already in Sunshine: {err}"),
        },
        None => println!(
            "Not logged in to Sunshine yet — games added from the Library from now on get \
             the new prep commands; re-add any added before to update them."
        ),
    }

    println!();
    println!("Done — restart Sunshine to start using it.");
    Ok(())
}

fn sunshine_conf_path() -> Result<PathBuf> {
    let home = directories::UserDirs::new()
        .context("no home directory")?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".config/sunshine/sunshine.conf"))
}

fn conf_backup_path(conf: &Path) -> PathBuf {
    conf.with_file_name("sunshine.conf.iprolaunch.bak")
}

fn conf_key_matches(line: &str, key: &str) -> bool {
    line.split_once('=')
        .is_some_and(|(k, _)| k.trim() == key && !line.trim_start().starts_with('#'))
}

/// Sets `key = value` in a sunshine.conf body: replaces the existing line or appends one.
fn set_conf_key(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            if !found && conf_key_matches(l, key) {
                found = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push(line);
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

fn which(name: &str) -> Option<String> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| p.is_file())
            .map(|p| p.display().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sunshine_devices_by_vendor_or_name() {
        assert!(is_sunshine_device(
            "4617:3:libvirtualhid_Mouse",
            "libvirtualhid Mouse",
            &[]
        ));
        assert!(is_sunshine_device(
            "48879:57005:Mouse_passthrough",
            "Mouse passthrough",
            &[]
        ));
        assert!(is_sunshine_device(
            "",
            "libvirtualhid-mouse-(absolute)",
            &[]
        ));
        assert!(!is_sunshine_device(
            "1133:49291:Logitech_G502",
            "Logitech G502",
            &[]
        ));
    }

    #[test]
    fn input_match_replaces_auto_detection() {
        let names = vec!["my-virtual-mouse".to_string()];
        assert!(is_sunshine_device("", "my-virtual-mouse", &names));
        assert!(!is_sunshine_device("", "libvirtualhid-mouse", &names));
    }

    #[test]
    fn channel_count_prefers_config_then_client() {
        let mut cfg = Sunshine::default();
        assert_eq!(channel_count(&cfg, Some("5.1")), 6);
        assert_eq!(channel_count(&cfg, Some("7.1")), 8);
        assert_eq!(channel_count(&cfg, None), 2);
        cfg.audio_channels = crate::config::AudioChannels::Stereo;
        assert_eq!(channel_count(&cfg, Some("7.1")), 2);
    }

    #[test]
    fn sway_config_disables_all_input_and_sets_mode() {
        let cfg = Sunshine {
            hide_cursor: true,
            ..Default::default()
        };
        let text = sway_config(&cfg);
        assert!(text.contains("output * mode 1920x1080@60Hz scale 1"));
        assert!(text.contains("input * events disabled"));
        assert!(text.contains("seat * hide_cursor 1"));
    }

    #[test]
    fn set_conf_key_replaces_or_appends() {
        let text = "output_name = 2\naudio_sink = old\n";
        assert_eq!(
            set_conf_key(text, "audio_sink", "iprolaunch-stream"),
            "output_name = 2\naudio_sink = iprolaunch-stream\n"
        );
        assert_eq!(
            set_conf_key("output_name = 2\n", "audio_sink", "x"),
            "output_name = 2\naudio_sink = x\n"
        );
        assert_eq!(set_conf_key("", "audio_sink", "x"), "audio_sink = x\n");
        assert_eq!(
            set_conf_key("# audio_sink = old\n", "audio_sink", "x"),
            "# audio_sink = old\naudio_sink = x\n"
        );
    }

    #[test]
    fn parses_sink_inputs_with_marker_and_pid() {
        let json: Value = serde_json::from_str(
            r#"[
                {"index": 7, "sink": 3, "properties": {"application.process.id": "4242", "iprolaunch.stream": "game"}},
                {"index": 9, "sink": 1, "properties": {"application.name": "Firefox"}}
            ]"#,
        )
        .unwrap();
        let inputs = parse_sink_inputs(&json);
        assert_eq!(inputs.len(), 2);
        assert_eq!((inputs[0].index, inputs[0].sink), (7, 3));
        assert_eq!(inputs[0].pid, Some(4242));
        assert!(inputs[0].marked);
        assert!(!inputs[1].marked && inputs[1].pid.is_none());
    }

    #[test]
    fn this_process_descends_from_its_parent_but_not_a_stranger() {
        let me = std::process::id();
        let parent = parent_pid(me).unwrap();
        assert!(descends_from(me, parent));
        assert!(descends_from(me, me));
        assert!(!descends_from(parent, me));
    }

    #[test]
    fn parses_busctl_string_arrays() {
        assert_eq!(
            parse_busctl_strings("as 2 \"event3\" \"event12\"\n"),
            vec!["event3", "event12"]
        );
        assert_eq!(
            parse_busctl_strings("s \"libvirtualhid-mouse\""),
            vec!["libvirtualhid-mouse"]
        );
    }

    #[test]
    fn override_conf_clears_then_sets_exec_start() {
        let text = override_conf(Path::new("/opt/iprolaunch"), "/usr/bin/sunshine");
        assert!(text.contains("ExecStart=\nExecStart=\"/opt/iprolaunch\" sunshine service"));
    }
}
