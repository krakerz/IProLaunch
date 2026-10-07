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

use crate::config::Sunshine;

fn base_url(cfg: &Sunshine) -> String {
    format!("https://{}:{}", cfg.host, cfg.port)
}

fn agent() -> Agent {
    Agent::config_builder()
        .tls_config(TlsConfig::builder().disable_verification(true).build())
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

/// `iprolaunch sunshine prep do|undo <slug>` for this exact binary.
fn prep_cmd_for(slug: &str) -> Result<PrepCmd> {
    let exe = std::env::current_exe()?.canonicalize()?;
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
    let mut parts = cmd.split_whitespace();
    let Some(first) = parts.next() else {
        return false;
    };
    let is_iprolaunch = std::path::Path::new(first.trim_matches('"'))
        .file_stem()
        .and_then(|s| s.to_str())
        == Some("iprolaunch");
    is_iprolaunch && parts.last() == Some(slug)
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
fn find_existing_index_in_apps(apps: &[ExistingApp], slug: &str) -> Option<i32> {
    apps.iter()
        .position(|a| matches_iprolaunch_slug(&a.cmd, slug))
        .map(|i| i as i32)
}

/// Looks up whether `slug` already has an app entry, returning its array
/// index if so. Best-effort: any failure here (network error, auth expired,
/// malformed response) just means `None` — `add_app` then falls back to
/// appending a new entry (`index: -1`) exactly like it always did, rather
/// than blocking the whole add over a lookup that couldn't complete. A
/// genuinely expired token still surfaces correctly to the caller regardless,
/// since the real `POST` right after this would hit the same 401.
fn find_existing_index(cfg: &Sunshine, auth_token: &str, slug: &str) -> Option<i32> {
    let url = format!("{}/api/apps", base_url(cfg));
    let response: AppsListResponse = agent()
        .get(&url)
        .header("Authorization", auth_token)
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    find_existing_index_in_apps(&response.apps, slug)
}

/// Registers `name`/`cmd` as a Sunshine app for `slug`, or updates it in
/// place if `slug` already has an entry (see `find_existing_index`) — so
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
) -> Result<bool, AddAppError> {
    let url = format!("{}/api/apps", base_url(cfg));
    let existing_index = find_existing_index(cfg, auth_token, slug);
    let prep_cmd = vec![prep_cmd_for(slug).map_err(AddAppError::Other)?];
    let payload = AddAppRequest {
        name,
        cmd,
        image_path: "",
        index: existing_index.unwrap_or(-1),
        auto_detach: true,
        wait_all: true,
        exit_timeout: 5,
        elevated: false,
        exclude_global_prep_cmd: false,
        output: "",
        prep_cmd,
        detached: Vec::new(),
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

/// Points every iprolaunch app already in Sunshine at this binary's current
/// prep commands, keeping all their other fields. Returns how many changed.
pub fn refresh_prep_cmds(cfg: &Sunshine, auth_token: &str) -> Result<usize, AddAppError> {
    let url = format!("{}/api/apps", base_url(cfg));
    let response = agent().get(&url).header("Authorization", auth_token).call();
    let mut body = match response {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(401)) => return Err(AddAppError::AuthExpired),
        Err(err) => return Err(AddAppError::Other(anyhow::Error::from(err))),
    };
    let list: serde_json::Value = body.body_mut().read_json().map_err(|e| {
        AddAppError::Other(anyhow::Error::from(e).context("reading Sunshine's app list"))
    })?;
    let apps = list
        .get("apps")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut updated = 0;
    for (index, mut app) in apps.into_iter().enumerate() {
        let Some(slug) = app
            .get("cmd")
            .and_then(serde_json::Value::as_str)
            .and_then(iprolaunch_slug)
            .map(str::to_string)
        else {
            continue;
        };
        let prep = prep_cmd_for(&slug).map_err(AddAppError::Other)?;
        let prep = serde_json::to_value(vec![prep]).map_err(|e| AddAppError::Other(e.into()))?;
        if app.get("prep-cmd") == Some(&prep) {
            continue;
        }
        app["prep-cmd"] = prep;
        app["index"] = serde_json::json!(index);
        match agent()
            .post(&url)
            .header("Authorization", auth_token)
            .send_json(&app)
        {
            Ok(_) => updated += 1,
            Err(ureq::Error::StatusCode(401)) => return Err(AddAppError::AuthExpired),
            Err(err) => {
                return Err(AddAppError::Other(
                    anyhow::Error::from(err).context(format!("updating \"{slug}\" in Sunshine")),
                ));
            }
        }
    }
    Ok(updated)
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

    fn existing_app(cmd: &str) -> ExistingApp {
        ExistingApp {
            cmd: cmd.to_string(),
        }
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
}
