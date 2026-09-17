//! Spawns a detached terminal emulator window to run a given command in —
//! used by `launch::run` to show a failed launch's log in its own window
//! instead of blocking whatever invoked iprolaunch (see that module for the
//! trigger conditions). There's no cross-desktop "the user's default
//! terminal" API, so this tries progressively less specific sources: the
//! freedesktop `xdg-terminal-exec` spec tool (confirmed via its own real
//! README: takes the command as bare positional args, no flag needed) when
//! installed, then the current desktop's own configured terminal (KDE's
//! `kdeglobals`, XFCE's `exo-open`, GNOME's gsettings key — each confirmed
//! against a real KDE dev machine rather than guessed: `kreadconfig6
//! --file kdeglobals --group General --key TerminalApplication` there
//! returned `/usr/bin/ghostty --gtk-single-instance=true`, not whatever a
//! hardcoded priority list would've guessed first), then a fixed priority
//! list of common terminal emulators (their own `-e`/`--`/bare-argv
//! conventions confirmed via each installed binary's own `--help`, except
//! gnome-terminal/xfce4-terminal/kitty/foot, none of which were installed
//! on that machine — those follow their own documented convention instead).
//! Any candidate that fails to spawn (not installed, unexpected flags) just
//! falls through to the next rather than erroring — this is a best-effort
//! convenience, never a required tool.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// How a given terminal emulator wants the command-to-run passed, keyed by
/// its binary's file name. Everything not listed is assumed to take `-e`
/// (confirmed the convention for xterm/konsole/alacritty/ghostty via their
/// own `--help`; xfce4-terminal follows the same convention per its own
/// docs, unconfirmed live since it isn't installed on the dev machine).
enum ExecStyle {
    /// `<term> -e <cmd> <args...>`, as the last argument.
    DashE,
    /// `<term> -- <cmd> <args...>` — gnome-terminal's own docs mark `-e` as
    /// deprecated/quote-fragile in favor of this.
    DoubleDash,
    /// `<term> <cmd> <args...>` with no flag at all (kitty, foot).
    BareArgv,
    /// `wezterm start -- <cmd> <args...>` — wezterm's own subcommand shape,
    /// confirmed via `wezterm start --help`.
    WeztermStart,
}

/// KDE's/GNOME's configured terminal value can be an absolute path (e.g.
/// `/usr/bin/ghostty ...`, confirmed on the dev machine) — `exec_style`
/// needs to match on the binary's own name, not the whole path.
fn basename(program: &str) -> &str {
    Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program)
}

fn exec_style(program_basename: &str) -> ExecStyle {
    match program_basename {
        "gnome-terminal" | "gnome-terminal.wrapper" => ExecStyle::DoubleDash,
        "kitty" | "foot" => ExecStyle::BareArgv,
        "wezterm" => ExecStyle::WeztermStart,
        _ => ExecStyle::DashE,
    }
}

/// Builds and spawns `program existing_args... <run-command-flags> pager
/// log_path` per `program`'s own `ExecStyle`. Returns whether the spawn
/// itself succeeded; doesn't (can't, being fire-and-forget) confirm the
/// window actually appeared or that `program` accepted the flags it was
/// given.
fn spawn_with_style(program: &str, existing_args: &[&str], pager: &str, log_path: &str) -> bool {
    let mut cmd = Command::new(program);
    cmd.args(existing_args);
    match exec_style(basename(program)) {
        ExecStyle::DashE => {
            cmd.arg("-e").arg(pager).arg(log_path);
        }
        ExecStyle::DoubleDash => {
            cmd.arg("--").arg(pager).arg(log_path);
        }
        ExecStyle::BareArgv => {
            cmd.arg(pager).arg(log_path);
        }
        ExecStyle::WeztermStart => {
            cmd.arg("start").arg("--").arg(pager).arg(log_path);
        }
    }
    spawn_detached(cmd)
}

/// Own process group, no inherited stdio — decouples the new terminal from
/// this process's controlling terminal/job control (so e.g. Ctrl+C on
/// iprolaunch's own shell, or `running::terminate`'s signal-forwarding
/// sweep, can't reach it) and from its lifetime (it outlives iprolaunch
/// exiting, same as any other orphaned child reparented to init).
fn spawn_detached(mut command: Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .is_ok()
}

fn try_xdg_terminal_exec(pager: &str, log_path: &str) -> bool {
    let mut cmd = Command::new("xdg-terminal-exec");
    cmd.arg(pager).arg(log_path);
    spawn_detached(cmd)
}

/// KDE's own configured default terminal — same value Dolphin's "Open
/// Terminal Here" uses. Split on whitespace rather than real shell-word
/// splitting — same "good enough, not a full parser" tradeoff as
/// `proton::steam_library_paths`'s VDF scan; a `TerminalApplication` value
/// is a simple binary+flags string in practice, never quoted-argument
/// shell syntax.
fn try_kde(pager: &str, log_path: &str) -> bool {
    for kreadconfig in ["kreadconfig6", "kreadconfig5"] {
        let Ok(output) = Command::new(kreadconfig)
            .args([
                "--file",
                "kdeglobals",
                "--group",
                "General",
                "--key",
                "TerminalApplication",
            ])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let mut parts = value.split_whitespace();
        let Some(program) = parts.next() else {
            continue;
        };
        let existing_args: Vec<&str> = parts.collect();
        if spawn_with_style(program, &existing_args, pager, log_path) {
            return true;
        }
    }
    false
}

/// XFCE's own preferred-application resolver — delegates XFCE's own
/// exec-syntax handling to `exo-open` itself rather than this module
/// guessing xfce4-terminal's flags directly.
fn try_xfce(pager: &str, log_path: &str) -> bool {
    let mut cmd = Command::new("exo-open");
    cmd.args(["--launch", "TerminalEmulator", pager, log_path]);
    spawn_detached(cmd)
}

/// GNOME's configured terminal — `gsettings get
/// org.gnome.desktop.default-applications.terminal exec`. On the dev
/// machine this resolved to the literal string `xdg-terminal-exec` (GNOME
/// itself deferring to that spec tool rather than naming a binary) — since
/// `try_xdg_terminal_exec` already ran first and would have caught that
/// case if the tool were actually installed, seeing that same value here
/// means it isn't, so there's nothing left to do but fall through.
fn try_gnome(pager: &str, log_path: &str) -> bool {
    let Ok(output) = Command::new("gsettings")
        .args([
            "get",
            "org.gnome.desktop.default-applications.terminal",
            "exec",
        ])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let value = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches('\'')
        .to_string();
    if value.is_empty() || value == "xdg-terminal-exec" {
        return false;
    }
    let mut parts = value.split_whitespace();
    let Some(program) = parts.next() else {
        return false;
    };
    let existing_args: Vec<&str> = parts.collect();
    spawn_with_style(program, &existing_args, pager, log_path)
}

/// Common terminal emulators, most to least likely to be the one actually
/// in use — last resort once `xdg-terminal-exec` and the current desktop's
/// own configured value (if any) have both come up empty.
const FALLBACK_TERMINALS: [&str; 8] = [
    "konsole",
    "gnome-terminal",
    "xfce4-terminal",
    "alacritty",
    "kitty",
    "foot",
    "wezterm",
    "xterm",
];

fn try_fallback_list(pager: &str, log_path: &str) -> bool {
    FALLBACK_TERMINALS
        .iter()
        .any(|program| spawn_with_style(program, &[], pager, log_path))
}

/// Tries, in priority order, to open `pager log_path` in a new terminal
/// window: the freedesktop spec tool, then the current desktop's own
/// configured terminal, then a fixed list of common ones. Returns whether
/// something was actually spawned — `false` means no terminal could be
/// found anywhere, which the caller treats as a plain skip (see
/// `launch::run`), not an error: this is a convenience, not a requirement.
pub fn spawn_in_new_terminal(pager: &str, log_path: &Path) -> bool {
    let log_path = log_path.to_string_lossy();

    if try_xdg_terminal_exec(pager, &log_path) {
        return true;
    }

    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();
    let de_specific = if desktop.contains("kde") {
        try_kde(pager, &log_path)
    } else if desktop.contains("xfce") {
        try_xfce(pager, &log_path)
    } else if desktop.contains("gnome") {
        try_gnome(pager, &log_path)
    } else {
        false
    };
    if de_specific {
        return true;
    }

    try_fallback_list(pager, &log_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_style_matches_known_binaries_and_defaults_to_dash_e() {
        assert!(matches!(
            exec_style("gnome-terminal"),
            ExecStyle::DoubleDash
        ));
        assert!(matches!(exec_style("kitty"), ExecStyle::BareArgv));
        assert!(matches!(exec_style("foot"), ExecStyle::BareArgv));
        assert!(matches!(exec_style("wezterm"), ExecStyle::WeztermStart));
        assert!(matches!(exec_style("konsole"), ExecStyle::DashE));
        assert!(matches!(
            exec_style("some-unknown-terminal"),
            ExecStyle::DashE
        ));
    }

    #[test]
    fn basename_strips_a_full_path_down_to_the_binary_name() {
        assert_eq!(basename("/usr/bin/gnome-terminal"), "gnome-terminal");
        assert_eq!(basename("konsole"), "konsole");
    }
}
