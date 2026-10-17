//! Sunshine "add app" integration — Library's Share/Add-to popup, Sunshine
//! option (see `tui/library.rs`). Talks directly to Sunshine's own local
//! REST API rather than shelling out to any third-party tool.
//!
//! Confirmed for real against a Sunshine install actually running on the
//! dev machine: an unauthenticated `GET /api/apps` returns a real `401
//! Unauthorized` JSON body (not a connection failure — the API is reachable
//! and does require auth), and a wrong-credential `Authorization: Basic ...`
//! against that same endpoint returns the identical 401 shape. That matches
//! vanilla Sunshine's actual, documented per-request auth model: its
//! `/api/*` endpoints accept plain HTTP Basic auth using the same username/
//! password set up in its own web UI, checked fresh on every request — no
//! separate login call or session cookie required. The `/api/login`-family
//! endpoints that a hedge-based approach (other Sunshine tools) tries in
//! sequence exist for the *browser* UI's own cookie session instead; they're
//! deliberately not used here (see project TODO/NOTES for the scoping
//! discussion — Basic auth alone is what's implemented for now).
//!
//! TLS certificate verification is disabled for every request: Sunshine's
//! local web UI is served over a self-signed certificate by default, with
//! no CA a user could install/trust instead.
//!
//! Every app gets a `prep-cmd` pointing back at `iprolaunch sunshine prep
//! do|undo <slug>` (see `vdesktop`), which fits the virtual desktop and the
//! audio sink to the connecting client.

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use ureq::Agent;
use ureq::tls::TlsConfig;

use crate::config::{Config, Sunshine};

fn base_url(cfg: &Sunshine) -> String {
    format!("https://{}:{}", cfg.host, cfg.port)
}

/// With a timeout: an unreachable `sunshine.host` would otherwise freeze the
/// TUI (status refresh at startup, add-to-Sunshine) for minutes.
fn agent() -> Agent {
    Agent::config_builder()
        .tls_config(TlsConfig::builder().disable_verification(true).build())
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent()
}

fn basic_auth_header(username: &str, password: &str) -> String {
    format!("Basic {}", BASE64.encode(format!("{username}:{password}")))
}

/// Confirms `username`/`password` against Sunshine's own `/api/apps` — a
/// harmless read, reused as the login check itself since there's no
/// dedicated login endpoint in play here (see this module's own doc
/// comment). Returns the `Basic ...` header value to cache on success.
pub fn login(cfg: &Sunshine, username: &str, password: &str) -> Result<String> {
    let token = basic_auth_header(username, password);
    let url = format!("{}/api/apps", base_url(cfg));
    match agent().get(&url).header("Authorization", &token).call() {
        Ok(_) => Ok(token),
        Err(ureq::Error::StatusCode(401)) => {
            bail!("Sunshine rejected that username/password")
        }
        Err(err) => Err(err).with_context(|| format!("contacting Sunshine at {url}")),
    }
}

/// One `prep-cmd` entry: a command run before the app starts (`do`) and its
/// matching teardown command once it stops (`undo`) — Sunshine's own real
/// field names, confirmed against a real Sunshine install's `GET /api/apps`
/// output (`do` inside each `prep-cmd` array entry, not a top-level
/// `cmd-do`/`cmd-undo` pair as an earlier, unverified guess might assume).
#[derive(Debug, Serialize)]
struct PrepCmd {
    #[serde(rename = "do")]
    do_cmd: String,
    undo: String,
}

/// The Sunshine app `cmd` for a profile: this binary, the profile's
/// Sunshine gamescope flags (if on), then its slug.
pub fn app_cmd(cfg: &Config, profile: &crate::config::Profile, slug: &str) -> Result<String> {
    let exe = crate::updater::own_exe()?.canonicalize()?;
    // Fullscreen gamescope with the game's window stretched to fill it, so
    // a fixed-size windowed game isn't a small box on a large virtual desktop.
    let flags: &[&str] = if cfg.effective_sunshine_gamescope(Some(profile)) {
        &["-f", "-w"]
    } else {
        &[]
    };
    Ok(crate::quick_launch_cmd::command_for(&exe, flags, slug))
}

/// `iprolaunch sunshine prep do|undo <slug>` for this exact binary.
fn prep_cmd_for(slug: &str) -> Result<PrepCmd> {
    let exe = crate::updater::own_exe()?.canonicalize()?;
    let base = format!("\"{}\" sunshine prep", exe.display());
    Ok(PrepCmd {
        do_cmd: format!("{base} do {slug}"),
        undo: format!("{base} undo {slug}"),
    })
}

/// Sunshine's own `POST /api/apps` payload shape — confirmed field-for-field
/// against a real Sunshine install's own `GET /api/apps` output (both a
/// third-party-managed entry and this integration's own earlier,
/// narrower payload), not left to guesswork: sending a narrower payload
/// (missing `elevated`/`exclude-global-prep-cmd`/`output`) leaves those
/// fields genuinely absent from the stored entry too, rather than
/// Sunshine filling in its own defaults — so parity with every other app in
/// a real Sunshine library means sending them explicitly here.
#[derive(Debug, Serialize)]
struct AddAppRequest<'a> {
    name: &'a str,
    cmd: &'a str,
    #[serde(rename = "image-path")]
    image_path: &'a str,
    index: i32,
    #[serde(rename = "auto-detach")]
    auto_detach: bool,
    #[serde(rename = "wait-all")]
    wait_all: bool,
    #[serde(rename = "exit-timeout")]
    exit_timeout: u32,
    elevated: bool,
    #[serde(rename = "exclude-global-prep-cmd")]
    exclude_global_prep_cmd: bool,
    output: &'a str,
    #[serde(rename = "prep-cmd")]
    prep_cmd: Vec<PrepCmd>,
    detached: Vec<()>,
}

/// Distinguishes an expired/invalid cached auth token (caller should clear
/// it and re-prompt for credentials) from every other failure (network
/// error, Sunshine down, malformed response, ...), which the caller should
/// just report as-is instead.
pub enum AddAppError {
    AuthExpired,
    Other(anyhow::Error),
}

impl std::fmt::Display for AddAppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddAppError::AuthExpired => write!(
                f,
                "Sunshine login has expired — add this game to Sunshine again to re-enter your username/password"
            ),
            AddAppError::Other(err) => write!(f, "{err:#}"),
        }
    }
}

/// The subset of an existing Sunshine app entry this needs to recognize a
/// re-add of the same game: just its own `cmd`.
#[derive(Debug, Deserialize)]
struct ExistingApp {
    #[serde(default)]
    cmd: String,
}

#[derive(Debug, Default, Deserialize)]
struct AppsListResponse {
    #[serde(default)]
    apps: Vec<ExistingApp>,
}

/// Whether `cmd` (an existing Sunshine app entry's own `cmd`) is this exact
/// `slug`'s iprolaunch entry — the *slug*, not the whole `cmd` string,
/// matters here: a literal full-string match (an earlier version of this
/// function used one) breaks the moment the command differs in any other
/// way that doesn't actually mean a different game — confirmed for real,
/// re-adding a game whose entry was originally created by a differently-
/// pathed iprolaunch build (a debug vs. release binary, or one moved/
/// reinstalled since) produced a genuine duplicate instead of updating,
/// since the two `cmd` strings' binary paths differed even though the slug
/// was identical. The same would happen if `sunshine.gamescope` changes
/// between adds (the baked-in `-f`/`-w` flag changes the `cmd` string too).
/// Mirrors `steam_shortcut::matching_slug`'s exact same reasoning (it has
/// the identical problem for Steam shortcuts, solved the same way there):
/// the first whitespace token's file stem must be `iprolaunch` (so an
/// unrelated app that happens to end in the same word never matches), and
/// the *last* whitespace token — always the slug, since any gamescope flags
/// `command_for` bakes in always come before it — must equal `slug` exactly.
fn matches_iprolaunch_slug(cmd: &str, slug: &str) -> bool {
    let Some((program, rest)) = split_program(cmd) else {
        return false;
    };
    let is_iprolaunch = std::path::Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        == Some("iprolaunch");
    is_iprolaunch && rest.split_whitespace().last() == Some(slug)
}

/// `(program, rest)` of a command line whose program may be `"quoted"` —
/// `command_for` quotes it, and the path can contain spaces.
fn split_program(cmd: &str) -> Option<(&str, &str)> {
    let cmd = cmd.trim_start();
    if let Some(quoted) = cmd.strip_prefix('"') {
        let end = quoted.find('"')?;
        return Some((&quoted[..end], &quoted[end + 1..]));
    }
    let end = cmd.find(char::is_whitespace).unwrap_or(cmd.len());
    (end > 0).then(|| (&cmd[..end], &cmd[end..]))
}

/// Pure lookup: the array position of the first entry in `apps` that's
/// already this `slug`'s own iprolaunch entry (see `matches_iprolaunch_slug`),
/// or `None` if this is a genuinely new game. Sunshine's own `POST
/// /api/apps` treats `index` as "the array position to overwrite" (confirmed
/// for real: re-posting an existing entry's own content at its real array
/// position left the total app count and that entry's position both
/// unchanged, rather than appending a second copy) — so this position is
/// exactly what `add_app` needs to update in place instead of duplicating.
/// Kept separate from the network call for testability.
fn find_existing_index_in_apps(apps: &[serde_json::Value], slug: &str) -> Option<usize> {
    apps.iter().position(|a| {
        a.get("cmd")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|c| matches_iprolaunch_slug(c, slug))
    })
}

/// Sunshine's raw app list. A failed lookup aborts the caller — appending
/// after a transient GET error would leave a duplicate entry behind.
fn fetch_apps(cfg: &Sunshine, auth_token: &str) -> Result<Vec<serde_json::Value>, AddAppError> {
    let url = format!("{}/api/apps", base_url(cfg));
    let mut response = match agent().get(&url).header("Authorization", auth_token).call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(401)) => return Err(AddAppError::AuthExpired),
        Err(err) => {
            return Err(AddAppError::Other(
                anyhow::Error::from(err).context(format!("reading Sunshine's app list at {url}")),
            ));
        }
    };
    let list: serde_json::Value = response.body_mut().read_json().map_err(|e| {
        AddAppError::Other(anyhow::Error::from(e).context("reading Sunshine's app list"))
    })?;
    Ok(list
        .get("apps")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Whether a `prep-cmd` entry is one iprolaunch wrote (current
/// `sunshine prep`, or the lutristosunshine display scripts older versions
/// reused) — everything else is the user's own and is kept.
fn is_our_prep_entry(entry: &serde_json::Value) -> bool {
    entry
        .get("do")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|d| d.contains(" sunshine prep ") || d.contains("lutristosunshine"))
}

/// `ours` first, then the user's own entries from `existing`, in order.
fn merge_prep_cmds(
    existing: Option<&serde_json::Value>,
    ours: serde_json::Value,
) -> serde_json::Value {
    let mut out = vec![ours];
    out.extend(
        existing
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|e| !is_our_prep_entry(e))
            .cloned(),
    );
    serde_json::Value::Array(out)
}

/// Whether iprolaunch may replace an entry's `image-path`: only when it's
/// unset or one of our own cached covers, never an image the user chose.
fn may_set_image(app: &serde_json::Value, covers_dir: Option<&std::path::Path>) -> bool {
    match app.get("image-path").and_then(serde_json::Value::as_str) {
        None | Some("") => true,
        Some(path) => covers_dir.is_some_and(|dir| std::path::Path::new(path).starts_with(dir)),
    }
}

/// Registers `name`/`cmd` as a Sunshine app for `slug`, or updates it in
/// place if `slug` already has an entry (see `find_existing_index_in_apps`) — so
/// re-adding the same profile a second time (a changed title, a rebuilt/
/// reinstalled iprolaunch binary at a new path, a changed `sunshine.gamescope`
/// baking in a different flag, or just re-running the popup) refreshes the
/// existing entry instead of leaving a duplicate behind. Returns whether an
/// existing entry was updated (`true`) or a new one was created (`false`),
/// so the caller can report the right one. `auth_token` is a cached `login`
/// result — callers are expected to `login` first the moment none is cached
/// yet, and to treat `AddAppError::AuthExpired` here as that cached token
/// having gone stale (Sunshine restarted with different credentials,
/// password changed, etc.), not as a permanent failure.
pub fn add_app(
    cfg: &Sunshine,
    auth_token: &str,
    name: &str,
    cmd: &str,
    slug: &str,
    image_path: &str,
) -> Result<bool, AddAppError> {
    let url = format!("{}/api/apps", base_url(cfg));
    let apps = fetch_apps(cfg, auth_token)?;
    let existing_index = find_existing_index_in_apps(&apps, slug);
    let prep = prep_cmd_for(slug).map_err(AddAppError::Other)?;
    let payload = match existing_index {
        // Update in place: only our own fields change, so whatever the user
        // customised in Sunshine's UI (timeouts, extra prep-cmds, image) stays.
        Some(index) => {
            let mut app = apps[index].clone();
            app["name"] = serde_json::json!(name);
            app["cmd"] = serde_json::json!(cmd);
            let ours = serde_json::to_value(prep).map_err(|e| AddAppError::Other(e.into()))?;
            app["prep-cmd"] = merge_prep_cmds(app.get("prep-cmd"), ours);
            let covers = crate::covers::covers_dir().ok();
            if !image_path.is_empty() && may_set_image(&app, covers.as_deref()) {
                app["image-path"] = serde_json::json!(image_path);
            }
            app["index"] = serde_json::json!(index);
            app
        }
        None => serde_json::to_value(AddAppRequest {
            name,
            cmd,
            image_path,
            index: -1,
            auto_detach: true,
            wait_all: true,
            exit_timeout: 5,
            elevated: false,
            exclude_global_prep_cmd: false,
            output: "",
            prep_cmd: vec![prep],
            detached: Vec::new(),
        })
        .map_err(|e| AddAppError::Other(e.into()))?,
    };
    match agent()
        .post(&url)
        .header("Authorization", auth_token)
        .send_json(&payload)
    {
        Ok(_) => Ok(existing_index.is_some()),
        Err(ureq::Error::StatusCode(401)) => Err(AddAppError::AuthExpired),
        Err(err) => Err(AddAppError::Other(
            anyhow::Error::from(err).context(format!("adding the app via {url}")),
        )),
    }
}

/// The slug an iprolaunch-made app `cmd` launches (its last token), or `None`
/// for any other app — same shape `matches_iprolaunch_slug` checks.
fn iprolaunch_slug(cmd: &str) -> Option<&str> {
    let slug = cmd.split_whitespace().last()?;
    matches_iprolaunch_slug(cmd, slug).then_some(slug)
}

/// Slugs of every iprolaunch game already in Sunshine; empty on any failure.
pub fn slugs_in_sunshine(cfg: &Sunshine, auth_token: &str) -> std::collections::HashSet<String> {
    let url = format!("{}/api/apps", base_url(cfg));
    let Ok(mut response) = agent().get(&url).header("Authorization", auth_token).call() else {
        return Default::default();
    };
    let Ok(list) = response.body_mut().read_json::<AppsListResponse>() else {
        return Default::default();
    };
    list.apps
        .iter()
        .filter_map(|a| iprolaunch_slug(&a.cmd).map(str::to_string))
        .collect()
}

/// What `sync_apps` changed.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub updated: usize,
    pub covers: usize,
    /// `(game, reason)` for each cover that couldn't be fetched.
    pub cover_errors: Vec<(String, String)>,
}

/// Brings every iprolaunch app already in Sunshine up to date: this binary's
/// current prep commands, and (with a SteamGridDB key) a cover image. All
/// other fields are kept, including the user's own prep-cmds and any image
/// they set themselves. `refetch_covers` re-downloads cached covers.
pub fn sync_apps(
    cfg: &Config,
    auth_token: &str,
    refetch_covers: bool,
) -> Result<SyncReport, AddAppError> {
    let url = format!("{}/api/apps", base_url(&cfg.sunshine));
    let apps = fetch_apps(&cfg.sunshine, auth_token)?;
    let key = cfg.steamgriddb.api_key.as_deref();
    let covers_dir = crate::covers::covers_dir().ok();

    let mut report = SyncReport::default();
    for (index, mut app) in apps.into_iter().enumerate() {
        let Some(slug) = app
            .get("cmd")
            .and_then(serde_json::Value::as_str)
            .and_then(iprolaunch_slug)
            .map(str::to_string)
        else {
            continue;
        };
        let mut changed = false;

        let profile = crate::config::Profile::load(&slug);
        if let Ok(profile) = &profile {
            let cmd = app_cmd(cfg, profile, &slug).map_err(AddAppError::Other)?;
            if app.get("cmd").and_then(serde_json::Value::as_str) != Some(cmd.as_str()) {
                app["cmd"] = serde_json::json!(cmd);
                changed = true;
            }
        }

        let prep = prep_cmd_for(&slug).map_err(AddAppError::Other)?;
        let prep = serde_json::to_value(prep).map_err(|e| AddAppError::Other(e.into()))?;
        let prep = merge_prep_cmds(app.get("prep-cmd"), prep);
        if app.get("prep-cmd") != Some(&prep) {
            app["prep-cmd"] = prep;
            changed = true;
        }

        let title = app
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&slug)
            .to_string();
        if may_set_image(&app, covers_dir.as_deref()) {
            let cover = crate::config::Profile::load(&slug)
                .and_then(|p| crate::covers::fetch_cover(key, &slug, &p, refetch_covers));
            match cover {
                Ok(path) => {
                    let path = serde_json::json!(path.display().to_string());
                    if app.get("image-path") != Some(&path) || refetch_covers {
                        app["image-path"] = path;
                        changed = true;
                        report.covers += 1;
                    }
                }
                Err(err) => report.cover_errors.push((title, format!("{err:#}"))),
            }
        }

        if !changed {
            continue;
        }
        app["index"] = serde_json::json!(index);
        match agent()
            .post(&url)
            .header("Authorization", auth_token)
            .send_json(&app)
        {
            Ok(_) => report.updated += 1,
            Err(ureq::Error::StatusCode(401)) => return Err(AddAppError::AuthExpired),
            Err(err) => {
                return Err(AddAppError::Other(
                    anyhow::Error::from(err).context(format!("updating \"{slug}\" in Sunshine")),
                ));
            }
        }
    }
    Ok(report)
}

/// Prints a `SyncReport` for the CLI / suspended-TUI actions.
pub fn print_sync_report(report: &SyncReport) {
    println!(
        "Updated {} game(s) in Sunshine ({} cover(s) set).",
        report.updated, report.covers
    );
    for (game, reason) in &report.cover_errors {
        println!("  no cover for \"{game}\": {reason}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_auth_header_base64_encodes_user_colon_pass() {
        assert_eq!(
            basic_auth_header("admin", "hunter2"),
            "Basic YWRtaW46aHVudGVyMg=="
        );
    }

    #[test]
    fn base_url_joins_host_and_port() {
        let cfg = Sunshine::default();
        assert_eq!(base_url(&cfg), "https://localhost:47990");
    }

    #[test]
    fn prep_cmd_serializes_do_as_the_bare_key_not_do_cmd() {
        let entry = PrepCmd {
            do_cmd: "/path/do".to_string(),
            undo: "/path/undo".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert_eq!(json, r#"{"do":"/path/do","undo":"/path/undo"}"#);
    }

    #[test]
    fn prep_cmd_calls_back_into_iprolaunch_with_the_slug() {
        let entry = prep_cmd_for("kendo").unwrap();
        assert!(entry.do_cmd.ends_with("\" sunshine prep do kendo"));
        assert!(entry.undo.ends_with("\" sunshine prep undo kendo"));
    }

    #[test]
    fn add_app_request_field_names_match_a_real_sunshine_install() {
        // Field-for-field parity check against the real shape confirmed by
        // fetching a real Sunshine install's own `GET /api/apps` — every
        // key here (image-path, auto-detach, wait-all, exit-timeout,
        // exclude-global-prep-cmd, prep-cmd) is byte-exact, not guessed.
        let payload = AddAppRequest {
            name: "Some Game",
            cmd: "\"/path/iprolaunch\" some-game",
            image_path: "",
            index: -1,
            auto_detach: true,
            wait_all: true,
            exit_timeout: 5,
            elevated: false,
            exclude_global_prep_cmd: false,
            output: "",
            prep_cmd: vec![PrepCmd {
                do_cmd: "/path/do".to_string(),
                undo: "/path/undo".to_string(),
            }],
            detached: Vec::new(),
        };
        let json = serde_json::to_string(&payload).unwrap();
        for key in [
            "\"name\"",
            "\"cmd\"",
            "\"image-path\"",
            "\"auto-detach\"",
            "\"wait-all\"",
            "\"exit-timeout\"",
            "\"elevated\"",
            "\"exclude-global-prep-cmd\"",
            "\"output\"",
            "\"prep-cmd\"",
            "\"do\"",
            "\"undo\"",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
    }

    fn existing_app(cmd: &str) -> serde_json::Value {
        serde_json::json!({ "cmd": cmd })
    }

    #[test]
    fn matches_iprolaunch_slug_ignores_flags_and_binary_path_differences() {
        // Real reported bug: an earlier version matched on the whole `cmd`
        // string, so re-adding a game whose entry was originally created by
        // a differently-pathed iprolaunch build (debug vs. release, or one
        // moved/reinstalled since) produced a genuine duplicate instead of
        // updating in place, even though it's unambiguously the same game.
        assert!(matches_iprolaunch_slug(
            "\"/home/user/target/debug/iprolaunch\" kendo",
            "kendo"
        ));
        assert!(matches_iprolaunch_slug(
            "\"/home/user/target/release/iprolaunch\" kendo",
            "kendo"
        ));
        // A gamescope flag baked in before the slug (see `command_for`) must
        // not break the match either — this is exactly what changes if
        // `sunshine.gamescope` is edited between two adds of the same game.
        assert!(matches_iprolaunch_slug(
            "\"/home/user/iprolaunch\" -f kendo",
            "kendo"
        ));
    }

    #[test]
    fn matches_iprolaunch_slug_requires_the_exe_stem_to_be_iprolaunch() {
        // An unrelated app (e.g. a Lutris-managed one) whose own `cmd`
        // happens to end in the same word as some slug must never match —
        // same defensive guard `steam_shortcut::matching_slug` already has.
        assert!(!matches_iprolaunch_slug(
            "/usr/bin/some-other-launcher kendo",
            "kendo"
        ));
    }

    #[test]
    fn matches_iprolaunch_slug_requires_the_exact_slug_as_the_last_token() {
        assert!(!matches_iprolaunch_slug(
            "\"/home/user/iprolaunch\" kendo-2",
            "kendo"
        ));
        assert!(!matches_iprolaunch_slug(
            "\"/home/user/iprolaunch\"",
            "kendo"
        ));
    }

    fn bare_profile() -> crate::config::Profile {
        crate::config::Profile {
            name: "Game#1".into(),
            target_path: "/tmp/game.exe".into(),
            title: None,
            gameid: None,
            last_launched: None,
            defaults: Default::default(),
            logging: Default::default(),
            env: Default::default(),
            winedlloverride: Default::default(),
            args: Vec::new(),
        }
    }

    #[test]
    fn app_cmd_defaults_to_stretched_fullscreen_gamescope_and_profile_can_turn_it_off() {
        let cfg = Config::default();
        let mut profile = bare_profile();
        let cmd = app_cmd(&cfg, &profile, "game").unwrap();
        assert!(cmd.ends_with("\" -f -w game"), "{cmd}");

        profile.defaults.sunshine_gamescope = Some(false);
        let cmd = app_cmd(&cfg, &profile, "game").unwrap();
        assert!(cmd.ends_with("\" game"), "{cmd}");
    }

    #[test]
    fn iprolaunch_slug_only_for_iprolaunch_apps() {
        assert_eq!(
            iprolaunch_slug("\"/opt/iprolaunch\" -f kendo"),
            Some("kendo")
        );
        assert_eq!(iprolaunch_slug("/usr/bin/steam steam://rungameid/1"), None);
        assert_eq!(iprolaunch_slug(""), None);
    }

    #[test]
    fn find_existing_index_in_apps_matches_by_slug_not_position() {
        let apps = vec![
            existing_app("\"/path/iprolaunch\" game-a"),
            existing_app("\"/path/iprolaunch\" game-b"),
            existing_app("\"/path/iprolaunch\" game-c"),
        ];
        assert_eq!(find_existing_index_in_apps(&apps, "game-b"), Some(1));
    }

    #[test]
    fn find_existing_index_in_apps_is_none_for_a_genuinely_new_slug() {
        let apps = vec![existing_app("\"/path/iprolaunch\" game-a")];
        assert_eq!(find_existing_index_in_apps(&apps, "game-z"), None);
    }

    #[test]
    fn find_existing_index_in_apps_is_none_for_an_empty_list() {
        assert_eq!(find_existing_index_in_apps(&[], "anything"), None);
    }

    #[test]
    fn matches_a_quoted_binary_path_containing_spaces() {
        let cmd = "\"/home/u/My Apps/iprolaunch\" -f kendo";
        assert!(matches_iprolaunch_slug(cmd, "kendo"));
        assert_eq!(iprolaunch_slug(cmd), Some("kendo"));
        assert!(!matches_iprolaunch_slug(
            "\"/home/u/My Apps/iprolaunch\"",
            "Apps/iprolaunch\""
        ));
        assert!(matches_iprolaunch_slug("/opt/iprolaunch kendo", "kendo"));
    }

    #[test]
    fn merge_prep_cmds_replaces_ours_and_keeps_the_users_own() {
        let existing = serde_json::json!([
            {"do": "\"/old/iprolaunch\" sunshine prep do game", "undo": "x"},
            {"do": "/home/u/.config/lutristosunshine/display/set.sh", "undo": "y"},
            {"do": "/usr/bin/my-own-script", "undo": ""},
        ]);
        let ours =
            serde_json::json!({"do": "\"/new/iprolaunch\" sunshine prep do game", "undo": "z"});
        let merged = merge_prep_cmds(Some(&existing), ours.clone());
        assert_eq!(
            merged,
            serde_json::json!([ours, {"do": "/usr/bin/my-own-script", "undo": ""}])
        );
        assert_eq!(
            merge_prep_cmds(None, ours.clone()),
            serde_json::json!([ours])
        );
    }

    #[test]
    fn may_set_image_only_when_unset_or_our_own_cover() {
        let dir = std::path::Path::new("/home/u/.local/share/iprolaunch/covers");
        assert!(may_set_image(&serde_json::json!({}), Some(dir)));
        assert!(may_set_image(
            &serde_json::json!({"image-path": ""}),
            Some(dir)
        ));
        assert!(may_set_image(
            &serde_json::json!({"image-path": "/home/u/.local/share/iprolaunch/covers/game.png"}),
            Some(dir)
        ));
        assert!(!may_set_image(
            &serde_json::json!({"image-path": "/home/u/Pictures/mine.png"}),
            Some(dir)
        ));
    }
}
