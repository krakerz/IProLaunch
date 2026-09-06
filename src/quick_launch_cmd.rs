use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// The quick-launch command line for a Steam non-Steam-game shortcut's
/// "Target" field — quoted binary path followed by the slug, matching
/// `main.rs`'s own quick-launch matching (`quick_launch` there accepts
/// either the slug or the display name — the slug's used here since it
/// can't collide the way a display name that got renamed could). Pure, so
/// it's unit-testable without a real clipboard; shared by the TUI's `c` key
/// and `add_profile`/`context-menu`'s auto-copy-on-add.
pub fn command_for(exe: &Path, slug: &str) -> String {
    format!("\"{}\" {slug}", exe.display())
}

/// Builds the command for `slug` against *this* running binary's own path
/// and copies it to the system clipboard.
pub fn copy_for_slug(slug: &str) -> Result<String> {
    let exe = std::env::current_exe().context("resolving iprolaunch's own binary path")?;
    let command = command_for(&exe, slug);
    copy_to_clipboard(&command)?;
    Ok(command)
}

/// Shells out to `wl-copy` (Wayland) or `xclip` (X11) rather than using a
/// Rust clipboard crate — both tools already do exactly what's needed here:
/// fork into the background and keep serving the clipboard selection after
/// the invoking process exits. That's not just a nicety, it's *required* on
/// both Wayland and X11 (whoever "owns" the clipboard selection has to stay
/// alive to answer paste requests) — confirmed for real: a Rust clipboard
/// crate's naive `set_text` then immediate-exit left the clipboard reading
/// back empty for a short-lived CLI call (`iprolaunch add`), while the
/// exact same call from the long-running TUI worked fine. Tries `wl-copy`
/// first (only when a Wayland session is actually running — the binary
/// existing isn't enough, e.g. an X11-only session with a leftover Wayland
/// dev package), then `xclip`.
fn copy_to_clipboard(text: &str) -> Result<()> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && run_copy_tool("wl-copy", &[], text).is_ok()
    {
        return Ok(());
    }
    if std::env::var_os("DISPLAY").is_some()
        && run_copy_tool("xclip", &["-selection", "clipboard"], text).is_ok()
    {
        return Ok(());
    }
    bail!("no clipboard tool available (tried wl-copy, xclip)")
}

fn run_copy_tool(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawning {program}"))?;
    {
        let stdin = child.stdin.take().context("no stdin handle")?;
        let mut stdin = stdin;
        stdin
            .write_all(text.as_bytes())
            .with_context(|| format!("writing to {program}'s stdin"))?;
        // Dropped here (end of block) so `program` sees EOF and can fork
        // into the background — `child.wait()` below would otherwise hang
        // waiting on a pipe we're still holding open.
    }
    let status = child
        .wait()
        .with_context(|| format!("waiting for {program}"))?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_for_quotes_the_binary_path_and_appends_the_slug() {
        assert_eq!(
            command_for(Path::new("/home/user/.local/bin/iprolaunch"), "eldenring"),
            "\"/home/user/.local/bin/iprolaunch\" eldenring"
        );
    }
}
