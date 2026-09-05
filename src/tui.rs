use anyhow::Result;

use crate::config::Config;

/// Placeholder — the ratatui screens (running/library/config/help) land in a
/// follow-up pass. For now, `iprolaunch run <target>` is the working path.
pub fn run(_cfg: Config) -> Result<()> {
    println!("iprolaunch: TUI not implemented yet — use `iprolaunch run <target>` for now.");
    Ok(())
}
