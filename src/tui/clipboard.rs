//! Best-effort system-clipboard write for the edit modal's Ctrl+C/Ctrl+X
//! (`modal::EditModal::pending_clipboard`, drained by
//! `App::handle_modal_event`).
//!
//! OSC 52 was tried first but verified (live, in a herdr pane) NOT to reach
//! the macOS clipboard through herdr — so this shells out to a platform tool
//! instead, writing the text to its stdin. Failure (tool missing, spawn
//! error, non-zero exit) is never fatal: the caller folds it into a status
//! message.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

/// Pick the platform clipboard-write command, given the two facts that
/// actually decide it: whether we're on macOS, and (on Linux) whether
/// `$WAYLAND_DISPLAY` is set. A pure function of those two inputs — kept
/// separate from `command()` (which reads them from the real environment) so
/// the choice itself is unit-testable without touching `cfg!` or spawning a
/// real process, mirroring `App::editor_cmd`'s "injected — no env mutation
/// in tests" pattern.
fn choose_command(
    is_macos: bool,
    wayland_display: Option<&str>,
) -> (&'static str, &'static [&'static str]) {
    if is_macos {
        return ("pbcopy", &[]);
    }
    match wayland_display {
        Some(v) if !v.is_empty() => ("wl-copy", &[]),
        _ => ("xclip", &["-selection", "clipboard"]),
    }
}

/// The real chooser, reading the actual platform/environment.
fn command() -> (&'static str, &'static [&'static str]) {
    choose_command(
        cfg!(target_os = "macos"),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
    )
}

/// Best-effort: write `text` to the system clipboard by spawning the chosen
/// tool and piping to its stdin. Never panics — every failure mode (tool not
/// on `$PATH`, spawn error, broken pipe, non-zero exit) comes back as `Err`
/// for the caller to turn into a status message.
pub fn copy(text: &str) -> Result<()> {
    let (bin, args) = command();
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawning {bin}"))?;
    child
        .stdin
        .take()
        .context("no stdin pipe")?
        .write_all(text.as_bytes())
        .with_context(|| format!("writing to {bin}"))?;
    let status = child.wait().with_context(|| format!("waiting on {bin}"))?;
    if !status.success() {
        bail!("{bin} exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_always_uses_pbcopy_regardless_of_wayland_display() {
        assert_eq!(choose_command(true, None), ("pbcopy", &[][..]));
        assert_eq!(choose_command(true, Some("wayland-0")), ("pbcopy", &[][..]));
    }

    #[test]
    fn linux_with_wayland_display_set_uses_wl_copy() {
        assert_eq!(
            choose_command(false, Some("wayland-0")),
            ("wl-copy", &[][..])
        );
    }

    #[test]
    fn linux_without_wayland_display_falls_back_to_xclip() {
        assert_eq!(
            choose_command(false, None),
            ("xclip", &["-selection", "clipboard"][..])
        );
        // An empty (but present) value doesn't count as "set".
        assert_eq!(
            choose_command(false, Some("")),
            ("xclip", &["-selection", "clipboard"][..])
        );
    }
}
