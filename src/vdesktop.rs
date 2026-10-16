//! Headless Sway "virtual desktop" for Sunshine streams.
//!
//! `service` is meant to replace Sunshine's own systemd `ExecStart`: it starts
//! a headless Sway, then runs Sunshine inside that Wayland display so Sunshine
//! captures only it, and games Sunshine launches inherit it (plus the stream's
//! audio sink). While running it keeps Sunshine's virtual input devices
//! enabled in the headless Sway and disabled on the host desktop. `prep` is
//! the per-app Sunshine `prep-cmd`: resizes the virtual output to the client
//! and creates the stream's audio sink.

use std::collections::{HashMap, HashSet};
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
/// Sunshine's own virtual input devices, as Hyprland names them. Disabled up
/// front so they're never live on the host, even before the first poll.
const SUNSHINE_DEVICE_NAMES: [&str; 6] = [
    "libvirtualhid-mouse",
    "libvirtualhid-mouse-(absolute)",
    "libvirtualhid-keyboard",
    "libvirtualhid-keyboard-1",
    "libvirtualhid-pen-tablet",
    "libvirtualhid-touchscreen",
];
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

/// Nothing on the virtual desktop gets a border or title bar. Sway floats
/// many game windows (fixed-size, dialog-type), and floating windows keep a
/// title bar unless `default_floating_border` says otherwise.
const NO_DECORATIONS: [&str; 3] = [
    "default_border none",
    "default_floating_border none",
    "for_window [all] border none",
];

pub fn sway_config(cfg: &Sunshine) -> String {
    let mut out = format!(
        "output * mode {}x{}@{}Hz scale {}\n\
         swaybg_command /usr/bin/true\n\
         input * events disabled\n",
        cfg.width, cfg.height, cfg.refresh, cfg.scale
    );
    for rule in NO_DECORATIONS {
        out.push_str(rule);
        out.push('\n');
    }
    out.push_str(&format!(
        "seat * hide_cursor {}\n",
        cfg.cursor.hide_timeout_ms()
    ));
    out
}

/// `wayland-*` socket name → inode. By inode, not just name: libwayland
/// reuses a stale socket's name (unlinks and rebinds it), so a previous
/// sway that died without cleaning up would hide the new one by name alone.
fn wayland_sockets() -> HashMap<String, u64> {
    use std::os::unix::fs::MetadataExt;
    fs::read_dir(runtime_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let ino = e.metadata().ok()?.ino();
                    Some((name, ino))
                })
                .filter(|(n, _)| n.starts_with("wayland-") && !n.ends_with(".lock"))
                .collect()
        })
        .unwrap_or_default()
}

/// A socket in `now` that wasn't in `before`, or was but has been re-created.
fn new_socket(before: &HashMap<String, u64>, now: HashMap<String, u64>) -> Option<String> {
    now.into_iter()
        .filter(|(name, ino)| before.get(name) != Some(ino))
        .map(|(name, _)| name)
        .min()
}

/// SIGTERM (so sway removes its sockets), then SIGKILL if it hasn't exited
/// within a few seconds. Plain `Child::kill` is SIGKILL, which leaves the
/// stale `wayland-N` behind.
fn stop_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: plain kill(2) on our own still-unreaped child's pid.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
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
fn wait_for_display(
    cfg: &Sunshine,
    before: &HashMap<String, u64>,
    sway: &mut Child,
) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(status) = sway.try_wait()? {
            bail!("headless sway exited during startup ({status})");
        }
        if let Some(display) = new_socket(before, wayland_sockets())
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

    let before = wayland_sockets();
    let mut sway = spawn_sway(s, &config_path)?;
    let display = match wait_for_display(s, &before, &mut sway) {
        Ok(d) => d,
        Err(err) => {
            stop_child(&mut sway);
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
    // systemd's user environment can hold a previous session's values; these
    // would make games think they're in that session (e.g. a stale
    // GAMESCOPE_WAYLAND_DISPLAY skips the gamescope wrap as "already nested").
    for var in STALE_SESSION_VARS {
        sunshine.env_remove(var);
    }
    let mut sunshine = match sunshine.spawn() {
        Ok(c) => c,
        Err(err) => {
            stop_child(&mut sway);
            return Err(err).with_context(|| format!("starting {sunshine_bin}"));
        }
    };

    // Stop cleanly on `systemctl stop`/restart, so host input gets re-enabled
    // and sway removes its sockets instead of being killed mid-way.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        let _ = signal_hook::flag::register(sig, std::sync::Arc::clone(&stop));
    }

    let mut watcher = InputWatcher::default();
    let (mut audio_events, mut subscriber) = subscribe_audio_events();
    let mut host_audio = HostAudioGuard::default();
    let result = loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            // Before stopping Sunshine: its devices must still exist to be
            // re-enabled on KDE/Sway hosts.
            watcher.restore_host(&s.input_match);
            stop_child(&mut sunshine);
            stop_child(&mut sway);
            break Ok(());
        }
        match sunshine.try_wait() {
            Ok(Some(status)) => {
                stop_child(&mut sway);
                if status.success() {
                    break Ok(());
                }
                break Err(anyhow::anyhow!("sunshine exited with {status}"));
            }
            Ok(None) => {}
            Err(err) => {
                stop_child(&mut sunshine);
                stop_child(&mut sway);
                break Err(err).context("waiting for sunshine");
            }
        }
        match sway.try_wait() {
            Ok(Some(status)) => {
                stop_child(&mut sunshine);
                break Err(anyhow::anyhow!("headless sway exited ({status})"));
            }
            Ok(None) => {}
            Err(err) => {
                stop_child(&mut sunshine);
                stop_child(&mut sway);
                break Err(err).context("waiting for headless sway");
            }
        }
        watcher.enable_in_virtual_desktop(s);
        if s.input_isolation {
            watcher.isolate_from_host(&swaysock(s), &s.input_match);
        }
        if let Err(err) = route_stream_audio(sunshine.id(), &s.audio_sink) {
            eprintln!("iprolaunch: audio routing: {err:#}");
        }
        if s.keep_host_audio {
            host_audio.keep_default(&s.audio_sink);
        }
        // Wake early when an audio stream appears, so a game that reopens its
        // output (e.g. on pause) is moved before it's audible on the host.
        match audio_events.recv_timeout(WATCH_INTERVAL) {
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // `pactl subscribe` is gone (missing, or pipewire-pulse
                // restarted): a dead channel returns instantly, so wait out the
                // interval here and try again, rather than spinning.
                std::thread::sleep(WATCH_INTERVAL);
                if let Some(mut child) = subscriber.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                (audio_events, subscriber) = subscribe_audio_events();
            }
            _ => while audio_events.try_recv().is_ok() {},
        }
    };
    if let Some(child) = subscriber.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    watcher.restore_host(&s.input_match);
    result
}

/// Host-session variables Sunshine (and the games it launches) must not inherit.
const STALE_SESSION_VARS: [&str; 5] = [
    "GAMESCOPE_WAYLAND_DISPLAY",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "I3SOCK",
    "NIRI_SOCKET",
    "XAUTHORITY",
];

/// Sunshine makes its own virtual sink the system default during a stream,
/// which would pull everything playing on the host into it. Game audio is
/// routed by `route_stream_audio` regardless, so the host default is put back.
#[derive(Default)]
struct HostAudioGuard {
    host_default: Option<String>,
}

fn is_stream_sink(name: &str, stream_sink: &str) -> bool {
    name == stream_sink || name.starts_with("sink-sunshine")
}

impl HostAudioGuard {
    fn keep_default(&mut self, stream_sink: &str) {
        let Some(current) = Command::new("pactl")
            .arg("get-default-sink")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        else {
            return;
        };
        if !is_stream_sink(&current, stream_sink) {
            self.host_default = Some(current);
            return;
        }
        if let Some(host) = &self.host_default {
            let ok = Command::new("pactl")
                .args(["set-default-sink", host])
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                // That sink is gone (unplugged); let the system pick again.
                self.host_default = None;
            }
        }
    }
}

/// Pulse/PipeWire stream property marking audio from games Sunshine launched.
const STREAM_PROP: &str = "iprolaunch.stream";

/// Spawns `pactl subscribe` and forwards a tick for each new/changed playback
/// stream. Without pactl (or once it exits) the channel is disconnected; the
/// service loop then falls back to plain polling and retries the subscriber.
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
                let relevant = (line.contains("sink-input")
                    && (line.contains("'new'") || line.contains("'change'")))
                    || (line.contains("'change'") && line.contains("server"));
                if relevant && tx.send(()).is_err() {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostDesktop {
    /// A live Hyprland instance's signature.
    Hyprland(String),
    Sway(PathBuf),
    Kde,
    Unknown,
}

fn socket_alive(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// The running Hyprland instance (newest one with a live control socket).
fn live_hyprland_instance() -> Option<String> {
    fs::read_dir(runtime_dir().join("hypr"))
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| socket_alive(&e.path().join(".socket.sock")))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .and_then(|e| e.file_name().into_string().ok())
}

/// A host Sway's IPC socket (`sway-ipc.*.sock`), never our own headless one.
fn live_host_sway(own: &Path) -> Option<PathBuf> {
    fs::read_dir(runtime_dir())
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p != own)
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("sway-ipc.") && n.ends_with(".sock"))
        })
        .find(|p| socket_alive(p))
}

impl HostDesktop {
    /// Probes what's actually running right now. The service's environment
    /// can't be trusted: systemd's user env keeps whatever the last desktop
    /// session exported (e.g. KDE vars while Hyprland is running).
    fn detect(own_swaysock: &Path) -> Self {
        if let Some(sig) = live_hyprland_instance() {
            return HostDesktop::Hyprland(sig);
        }
        if let Some(sock) = live_host_sway(own_swaysock) {
            return HostDesktop::Sway(sock);
        }
        let kwin = busctl_get(
            "/org/kde/KWin/InputDevice",
            "org.kde.KWin.InputDeviceManager",
            "devicesSysNames",
        );
        if kwin.is_ok() {
            return HostDesktop::Kde;
        }
        HostDesktop::Unknown
    }
}

/// Forwards a tick whenever Hyprland reloads its config, which drops rules
/// added at runtime with `hyprctl eval`. Ends when that instance goes away.
fn watch_hyprland_reloads(signature: &str, tx: std::sync::mpsc::Sender<()>) {
    use std::io::{BufRead, BufReader};
    let path = runtime_dir()
        .join("hypr")
        .join(signature)
        .join(".socket2.sock");
    let Ok(stream) = std::os::unix::net::UnixStream::connect(path) else {
        return;
    };
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if line.starts_with("configreloaded") && tx.send(()).is_err() {
                break;
            }
        }
    });
}

/// Remembers what's already been switched so each device is touched once
/// while it exists, not every poll.
struct InputWatcher {
    enabled: HashSet<String>,
    host: HostDesktop,
    host_disabled: HashSet<String>,
    reloads_tx: std::sync::mpsc::Sender<()>,
    reloads: std::sync::mpsc::Receiver<()>,
    restored: bool,
}

impl Default for InputWatcher {
    fn default() -> Self {
        let (reloads_tx, reloads) = std::sync::mpsc::channel();
        Self {
            enabled: HashSet::new(),
            host: HostDesktop::Unknown,
            host_disabled: HashSet::new(),
            reloads_tx,
            reloads,
            restored: false,
        }
    }
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

    fn isolate_from_host(&mut self, own_swaysock: &Path, match_names: &[String]) {
        let host = HostDesktop::detect(own_swaysock);
        if host != self.host {
            if host == HostDesktop::Unknown {
                eprintln!(
                    "iprolaunch: no Hyprland, Sway or KDE session found — streamed input \
                     may also reach the host desktop"
                );
            } else {
                eprintln!("iprolaunch: isolating streamed input from {host:?}");
            }
            if let HostDesktop::Hyprland(sig) = &host {
                watch_hyprland_reloads(sig, self.reloads_tx.clone());
            }
            self.host = host.clone();
            self.host_disabled.clear();
        }
        if self.reloads.try_iter().count() > 0 {
            self.host_disabled.clear();
        }
        let result = match &host {
            HostDesktop::Hyprland(sig) => self.isolate_hyprland(sig, match_names),
            HostDesktop::Sway(sock) => self.isolate_sway(sock, match_names),
            HostDesktop::Kde => self.isolate_kde(match_names),
            HostDesktop::Unknown => Ok(()),
        };
        if let Err(err) = result {
            eprintln!("iprolaunch: host input isolation: {err:#}");
        }
    }

    /// Hyprland keeps a device rule even for a device that doesn't exist yet,
    /// so Sunshine's known devices are disabled up front (re-done after a
    /// config reload); anything else matching is caught when it appears.
    fn isolate_hyprland(&mut self, sig: &str, match_names: &[String]) -> Result<()> {
        let mut names: Vec<String> = if match_names.is_empty() {
            SUNSHINE_DEVICE_NAMES
                .iter()
                .map(|n| n.to_string())
                .collect()
        } else {
            match_names.to_vec()
        };
        let out = hyprctl(sig)
            .args(["devices", "-j"])
            .output()
            .context("running hyprctl")?;
        let json: Value = serde_json::from_slice(&out.stdout).context("parsing hyprctl devices")?;
        names.extend(
            ["mice", "keyboards", "tablets", "touch"]
                .iter()
                .filter_map(|k| json.get(*k).and_then(Value::as_array))
                .flatten()
                .filter_map(|d| d.get("name").and_then(Value::as_str))
                .filter(|n| is_sunshine_device("", n, match_names))
                .map(str::to_string),
        );
        for name in names {
            if self.host_disabled.contains(&name) {
                continue;
            }
            hyprland_set_enabled(sig, &name, false)?;
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
            if kde_set_enabled(&sys, false)? {
                self.host_disabled.insert(sys);
            }
        }
        Ok(())
    }

    /// Re-enables Sunshine's devices on the host when the service stops, so a
    /// later plain Sunshine (desktop streaming) still gets input. Once only.
    fn restore_host(&mut self, match_names: &[String]) {
        if self.restored || self.host == HostDesktop::Unknown {
            return;
        }
        self.restored = true;
        if let Err(err) = set_host_input(&self.host, match_names, true) {
            eprintln!("iprolaunch: re-enabling host input: {err:#}");
        }
        self.host_disabled.clear();
    }
}

/// Enables/disables every Sunshine device currently known to `host` (on
/// Hyprland also the known names, whose rules outlive the devices).
fn set_host_input(host: &HostDesktop, match_names: &[String], enabled: bool) -> Result<()> {
    match host {
        HostDesktop::Hyprland(sig) => {
            let mut names: Vec<String> = if match_names.is_empty() {
                SUNSHINE_DEVICE_NAMES
                    .iter()
                    .map(|n| n.to_string())
                    .collect()
            } else {
                match_names.to_vec()
            };
            if let Ok(out) = hyprctl(sig).args(["devices", "-j"]).output()
                && let Ok(json) = serde_json::from_slice::<Value>(&out.stdout)
            {
                names.extend(
                    ["mice", "keyboards", "tablets", "touch"]
                        .iter()
                        .filter_map(|k| json.get(*k).and_then(Value::as_array))
                        .flatten()
                        .filter_map(|d| d.get("name").and_then(Value::as_str))
                        .filter(|n| is_sunshine_device("", n, match_names))
                        .map(str::to_string),
                );
            }
            names.sort();
            names.dedup();
            for name in names {
                hyprland_set_enabled(sig, &name, enabled)?;
            }
        }
        HostDesktop::Sway(sock) => {
            let state = if enabled { "enabled" } else { "disabled" };
            for (id, name, _) in sway_inputs(sock)? {
                if is_sunshine_device(&id, &name, match_names) {
                    let quoted = format!("\"{}\"", id.replace('"', "\\\""));
                    swaymsg(sock, &format!("input {quoted} events {state}"))?;
                }
            }
        }
        HostDesktop::Kde => {
            let devices = busctl_get(
                "/org/kde/KWin/InputDevice",
                "org.kde.KWin.InputDeviceManager",
                "devicesSysNames",
            )?;
            for sys in parse_busctl_strings(&devices) {
                let path = format!("/org/kde/KWin/InputDevice/{sys}");
                let name = busctl_get(&path, "org.kde.KWin.InputDevice", "name")
                    .map(|raw| {
                        parse_busctl_strings(&raw)
                            .into_iter()
                            .next()
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                if is_sunshine_device("", &name, match_names) {
                    kde_set_enabled(&sys, enabled)?;
                }
            }
        }
        HostDesktop::Unknown => {}
    }
    Ok(())
}

/// Sets a KWin input device's `enabled`; `Ok(false)` if busctl refused.
fn kde_set_enabled(sys: &str, enabled: bool) -> Result<bool> {
    let path = format!("/org/kde/KWin/InputDevice/{sys}");
    let status = Command::new("busctl")
        .args(["--user", "set-property", "org.kde.KWin", &path])
        .args(["org.kde.KWin.InputDevice", "enabled", "b"])
        .arg(if enabled { "true" } else { "false" })
        .status()
        .context("running busctl")?;
    Ok(status.success())
}

/// `hyprctl` pinned to one instance, since the inherited signature may be stale.
fn hyprctl(signature: &str) -> Command {
    let mut cmd = Command::new("hyprctl");
    cmd.env("HYPRLAND_INSTANCE_SIGNATURE", signature);
    cmd
}

/// Hyprland 0.56+ is Lua-configured (`keyword` is rejected there); older
/// builds only have `keyword`.
fn hyprland_set_enabled(sig: &str, name: &str, enabled: bool) -> Result<()> {
    let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
    let value = if enabled { "true" } else { "false" };
    let lua = format!("hl.device({{ name = \"{escaped}\", enabled = {value} }})");
    if hyprctl_ok(sig, &["eval", &lua]) {
        return Ok(());
    }
    if hyprctl_ok(sig, &["keyword", &format!("device[{name}]:enabled"), value]) {
        return Ok(());
    }
    let verb = if enabled { "enable" } else { "disable" };
    bail!("hyprctl couldn't {verb} `{name}`")
}

/// hyprctl exits 0 even on a rejected command, so judge by its reply text.
fn hyprctl_ok(sig: &str, args: &[&str]) -> bool {
    hyprctl(sig).args(args).output().is_ok_and(|o| {
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

/// Applies `cursor` at stream start, so it takes effect without restarting
/// Sunshine. Sway's hide timer only runs after pointer activity, which never
/// happens with a controller — parking the cursor in the corner counts as
/// activity, so the timer starts.
fn apply_cursor(cfg: &Sunshine, mode: StreamMode) -> Result<()> {
    let sock = swaysock(cfg);
    let timeout = cfg.cursor.hide_timeout_ms();
    swaymsg(&sock, &format!("seat * hide_cursor {timeout}"))?;
    if timeout > 0 {
        swaymsg(
            &sock,
            &format!(
                "seat * cursor set {} {}",
                mode.width.saturating_sub(1),
                mode.height.saturating_sub(1)
            ),
        )?;
    }
    Ok(())
}

fn sink_module_file() -> PathBuf {
    state_dir().join("sink-module-id")
}

/// Indices of the loaded null-sink modules that create `sink_name`.
fn our_sink_modules(modules: &Value, sink_name: &str) -> Vec<u64> {
    let wanted = format!("sink_name={sink_name}");
    modules
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m.get("name").and_then(Value::as_str) == Some("module-null-sink"))
        .filter(|m| {
            m.get("argument")
                .and_then(Value::as_str)
                .is_some_and(|a| a.split_whitespace().any(|t| t == wanted))
        })
        .filter_map(|m| m.get("index").and_then(Value::as_u64))
        .collect()
}

/// Unloads the stream sink by matching the module itself: a saved module id
/// can outlive a pipewire-pulse restart and then name someone else's module.
fn unload_sink(cfg: &Sunshine) {
    let ids: Vec<String> = match pactl_json(&["list", "modules"]) {
        Ok(json) => our_sink_modules(&json, &cfg.audio_sink)
            .iter()
            .map(u64::to_string)
            .collect(),
        // pactl without JSON output: fall back to the id saved at load time.
        Err(_) => fs::read_to_string(sink_module_file())
            .map(|id| vec![id.trim().to_string()])
            .unwrap_or_default(),
    };
    for id in ids.iter().filter(|id| !id.is_empty()) {
        let _ = Command::new("pactl").args(["unload-module", id]).status();
    }
    let _ = fs::remove_file(sink_module_file());
}

fn load_sink(cfg: &Sunshine, channels: u32) -> Result<()> {
    unload_sink(cfg);
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
            report("applying the cursor setting", apply_cursor(s, mode));
            for rule in NO_DECORATIONS {
                report("removing window decorations", swaymsg(&swaysock(s), rule));
            }
            let client_audio = std::env::var("SUNSHINE_CLIENT_AUDIO_CONFIGURATION").ok();
            let channels = channel_count(s, client_audio.as_deref());
            report("creating the audio sink", load_sink(s, channels));
        }
        PrepAction::Undo => {
            let mode = cfg.effective_stream_mode(None, None);
            report("resetting the virtual desktop", set_output_mode(s, mode));
            unload_sink(s);
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

const OUR_MARKER: &str = "Written by iprolaunch";
/// Our own drop-in, sorted after `override.conf` so our `ExecStart` wins,
/// and leaving the user's own `override.conf` untouched.
const OUR_DROPIN: &str = "zz-iprolaunch.conf";

fn dropin_dir(unit: &str) -> Result<PathBuf> {
    let home = directories::UserDirs::new()
        .context("no home directory")?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".config/systemd/user").join(format!("{unit}.d")))
}

fn is_ours(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|s| s.contains(OUR_MARKER))
}

/// Whether the service is set up by iprolaunch (current drop-in, or an older
/// version's `override.conf`).
fn is_installed(unit: &str) -> bool {
    dropin_dir(unit)
        .is_ok_and(|d| is_ours(&d.join(OUR_DROPIN)) || is_ours(&d.join("override.conf")))
}

/// Older versions replaced `override.conf` itself (backing the user's up to
/// `override.conf.bak`). Puts the user's back, or removes ours. Returns
/// whether there was anything to undo.
fn undo_legacy_override(dir: &Path) -> Result<bool> {
    let path = dir.join("override.conf");
    if !is_ours(&path) {
        return Ok(false);
    }
    let backup = dir.join("override.conf.bak");
    if backup.exists() {
        fs::rename(&backup, &path).with_context(|| format!("restoring {}", backup.display()))?;
        println!("Restored {} from its backup.", path.display());
    } else {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        println!("Removed the old iprolaunch {}.", path.display());
    }
    Ok(true)
}

/// One-line state of the Sunshine service setup, for the TUI.
pub fn service_status() -> String {
    let Some(unit) = find_sunshine_unit() else {
        return "Sunshine service not found".to_string();
    };
    let ours = is_installed(unit);
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

/// Undoes `install_service`: removes its drop-in (and an older version's
/// `override.conf`), and puts back only the `audio_sink` it changed in
/// sunshine.conf — every other setting changed since install is kept.
pub fn restore_service() -> Result<()> {
    let unit = find_sunshine_unit().context("no Sunshine systemd user service found")?;
    let dir = dropin_dir(unit)?;
    let mut restored = undo_legacy_override(&dir)?;
    let ours = dir.join(OUR_DROPIN);
    if ours.exists() {
        fs::remove_file(&ours).with_context(|| format!("removing {}", ours.display()))?;
        println!(
            "Removed {} — Sunshine's service is back to its default.",
            ours.display()
        );
        restored = true;
    }
    let _ = fs::remove_dir(&dir); // only succeeds when empty
    let conf = sunshine_conf_path()?;
    let conf_backup = conf_backup_path(&conf);
    if let Ok(snapshot) = fs::read_to_string(&conf_backup) {
        let current = fs::read_to_string(&conf).unwrap_or_default();
        let text = match get_conf_key(&snapshot, "audio_sink") {
            Some(previous) => set_conf_key(&current, "audio_sink", &previous),
            None => remove_conf_key(&current, "audio_sink"),
        };
        crate::config::write_atomic(&conf, text.as_bytes())?;
        let _ = fs::remove_file(&conf_backup);
        println!("Restored audio_sink in {}.", conf.display());
        restored = true;
    }
    // Hyprland keeps our runtime "disabled" rules for Sunshine's devices until
    // a config reload; a plain Sunshine would get no input on this host.
    let sunshine = Config::load_or_init()
        .map(|c| c.sunshine)
        .unwrap_or_default();
    let host = HostDesktop::detect(&swaysock(&sunshine));
    if let Err(err) = set_host_input(&host, &sunshine.input_match, true) {
        println!("Couldn't re-enable Sunshine's input devices on this desktop: {err:#}");
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

/// After a self-update: the service still runs the old binary, so restart it
/// when it's ours and running. Ends an active stream, hence the notice.
pub fn restart_after_update() {
    let Some(unit) = find_sunshine_unit() else {
        return;
    };
    let ours = is_installed(unit);
    let active = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", unit])
        .status()
        .is_ok_and(|s| s.success());
    if !ours || !active {
        return;
    }
    println!(
        "Restarting the Sunshine service so it runs the new version (ends any active stream)..."
    );
    if let Err(e) = restart_service() {
        println!("Couldn't restart the Sunshine service: {e:#}. Restart it from the Sunshine tab.");
    }
}

/// Points Sunshine's systemd user service at `iprolaunch sunshine service`
/// via its own drop-in, leaving any user `override.conf` in place (see
/// `restore_service`).
pub fn install_service(cfg: &Config) -> Result<()> {
    let unit = find_sunshine_unit()
        .context("no Sunshine systemd user service found (looked for app-dev.lizardbyte.app.Sunshine.service and sunshine.service)")?;
    let sunshine_bin = which("sunshine").context("`sunshine` isn't on $PATH")?;
    let bin = crate::updater::own_exe()?.canonicalize()?;
    let dropin = dropin_dir(unit)?;
    fs::create_dir_all(&dropin)?;
    undo_legacy_override(&dropin)?;
    let path = dropin.join(OUR_DROPIN);
    crate::config::write_atomic(&path, override_conf(&bin, &sunshine_bin).as_bytes())?;
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
    crate::config::write_atomic(
        &conf,
        set_conf_key(&text, "audio_sink", &cfg.sunshine.audio_sink).as_bytes(),
    )?;
    println!(
        "Set audio_sink = {} in {}",
        cfg.sunshine.audio_sink,
        conf.display()
    );

    println!();
    match cfg.sunshine.auth_token.as_deref() {
        Some(token) => match crate::sunshine::sync_apps(cfg, token, false) {
            Ok(report) => crate::sunshine::print_sync_report(&report),
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

/// The value of `key` in a sunshine.conf body, if set.
fn get_conf_key(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find(|l| conf_key_matches(l, key))
        .and_then(|l| l.split_once('='))
        .map(|(_, v)| v.trim().to_string())
}

/// Drops every `key = ...` line from a sunshine.conf body.
fn remove_conf_key(text: &str, key: &str) -> String {
    let mut out: String = text
        .lines()
        .filter(|l| !conf_key_matches(l, key))
        .collect::<Vec<_>>()
        .join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
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
    fn known_device_names_are_all_detected_as_sunshine() {
        for name in SUNSHINE_DEVICE_NAMES {
            assert!(is_sunshine_device("", name, &[]), "{name}");
        }
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
            cursor: crate::config::CursorMode::Hidden,
            ..Default::default()
        };
        let text = sway_config(&cfg);
        assert!(sway_config(&Sunshine::default()).contains("seat * hide_cursor 3000"));
        assert!(text.contains("output * mode 1920x1080@60Hz scale 1"));
        assert!(text.contains("input * events disabled"));
        assert!(text.contains("default_floating_border none"));
        assert!(text.contains("for_window [all] border none"));
        assert!(text.contains("seat * hide_cursor 1"));
    }

    #[test]
    fn stream_sinks_are_ours_and_sunshines() {
        assert!(is_stream_sink("iprolaunch-stream", "iprolaunch-stream"));
        assert!(is_stream_sink("sink-sunshine-stereo", "iprolaunch-stream"));
        assert!(is_stream_sink(
            "sink-sunshine-surround71",
            "iprolaunch-stream"
        ));
        assert!(!is_stream_sink(
            "alsa_output.pci-0000_58_00.6.analog-stereo",
            "iprolaunch-stream"
        ));
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
    fn get_and_remove_conf_key() {
        let text = "output_name = 2\naudio_sink = old\n# audio_sink = c\n";
        assert_eq!(get_conf_key(text, "audio_sink").as_deref(), Some("old"));
        assert_eq!(get_conf_key("# audio_sink = c\n", "audio_sink"), None);
        assert_eq!(
            remove_conf_key(text, "audio_sink"),
            "output_name = 2\n# audio_sink = c\n"
        );
        assert_eq!(remove_conf_key("audio_sink = x\n", "audio_sink"), "");
    }

    #[test]
    fn new_socket_spots_a_recreated_name_and_ignores_unchanged_ones() {
        let before = HashMap::from([("wayland-0".to_string(), 10), ("wayland-1".to_string(), 11)]);
        let same = before.clone();
        assert_eq!(new_socket(&before, same), None);
        let rebound = HashMap::from([("wayland-0".to_string(), 10), ("wayland-1".to_string(), 99)]);
        assert_eq!(new_socket(&before, rebound).as_deref(), Some("wayland-1"));
        let added = HashMap::from([("wayland-0".to_string(), 10), ("wayland-2".to_string(), 5)]);
        assert_eq!(new_socket(&before, added).as_deref(), Some("wayland-2"));
    }

    #[test]
    fn finds_only_our_null_sink_modules() {
        let json: Value = serde_json::from_str(
            r#"[
                {"index": 4, "name": "module-null-sink", "argument": "sink_name=iprolaunch-stream channels=2"},
                {"index": 5, "name": "module-null-sink", "argument": "sink_name=iprolaunch-stream-other"},
                {"index": 6, "name": "module-echo-cancel", "argument": "sink_name=iprolaunch-stream"}
            ]"#,
        )
        .unwrap();
        assert_eq!(our_sink_modules(&json, "iprolaunch-stream"), vec![4]);
    }

    #[test]
    fn override_conf_clears_then_sets_exec_start() {
        let text = override_conf(Path::new("/opt/iprolaunch"), "/usr/bin/sunshine");
        assert!(text.contains("ExecStart=\nExecStart=\"/opt/iprolaunch\" sunshine service"));
    }
}
