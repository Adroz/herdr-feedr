//! Best-effort system-clipboard write for the edit modal's copies
//! (`modal::EditModal::pending_clipboard`, drained by
//! `App::handle_modal_event`).
//!
//! Two paths, both always tried: an OSC 52 sequence on the terminal, then a
//! platform tool (pbcopy / wl-copy / xclip) fed on stdin. OSC 52 does pass
//! through herdr (0.9.x intercepts it from pane children and re-emits it via
//! pbcopy on the foreground client; verified live), and it is the path that
//! works when herdr or feedr runs remotely. Failure is never fatal: the
//! caller folds it into a status message.

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

/// Where the sidebar's copies go. Injected into `App` so tests record
/// writes instead of touching the developer's real clipboard.
pub trait ClipboardSink {
    fn copy(&mut self, text: &str) -> Result<()>;
}

/// OSC 52 "set clipboard" for `text`: target `c`, base64 payload,
/// BEL-terminated (herdr and most terminals accept BEL and ST alike).
fn osc52(text: &str) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{b64}\x07")
}

/// Production sink: OSC 52 to the terminal, then the platform tool
/// (`copy`). Two paths because each covers what the other can't: OSC 52
/// reaches the clipboard of whichever machine the terminal (or herdr's
/// foreground client) runs on, so it works over SSH; the platform tool works
/// in terminals that ignore OSC 52. Either may fail without blocking the
/// other; the copy fails only if both do.
pub struct SystemClipboard<W = std::io::Stdout, T = fn(&str) -> Result<()>> {
    /// Where the OSC 52 sequence goes: the terminal feedr draws on.
    pub term: W,
    tool: T,
}

impl SystemClipboard {
    pub fn new() -> Self {
        Self::with(std::io::stdout(), copy)
    }
}

impl Default for SystemClipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl<W: Write, T: FnMut(&str) -> Result<()>> SystemClipboard<W, T> {
    pub fn with(term: W, tool: T) -> Self {
        SystemClipboard { term, tool }
    }
}

impl<W: Write, T: FnMut(&str) -> Result<()>> ClipboardSink for SystemClipboard<W, T> {
    fn copy(&mut self, text: &str) -> Result<()> {
        let osc = self
            .term
            .write_all(osc52(text).as_bytes())
            .and_then(|()| self.term.flush());
        let tool = (self.tool)(text);
        match (osc, tool) {
            (Err(o), Err(t)) => bail!("OSC 52: {o}; {t}"),
            _ => Ok(()),
        }
    }
}

/// Records every write; clones share the record. `fail` makes each write
/// error (after recording it), for exercising the status-message path.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct FakeClipboard {
    pub writes: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    pub fail: bool,
}

#[cfg(test)]
impl ClipboardSink for FakeClipboard {
    fn copy(&mut self, text: &str) -> Result<()> {
        self.writes.borrow_mut().push(text.to_string());
        if self.fail {
            bail!("fake clipboard failure");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Calls = std::rc::Rc<std::cell::RefCell<Vec<String>>>;

    /// A production sink over an in-memory terminal and a tool recording
    /// into `calls`.
    fn sink(
        tool_fails: bool,
        calls: &Calls,
    ) -> SystemClipboard<Vec<u8>, impl FnMut(&str) -> Result<()>> {
        let rec = calls.clone();
        let tool = move |t: &str| {
            rec.borrow_mut().push(t.to_string());
            if tool_fails {
                bail!("no pbcopy");
            }
            Ok(())
        };
        SystemClipboard::with(Vec::new(), tool)
    }

    #[test]
    fn the_system_sink_writes_osc_52_then_runs_the_platform_tool() {
        let calls = Calls::default();
        let mut s = sink(false, &calls);
        s.copy("héllo\nworld").unwrap();

        // base64("héllo\nworld") as UTF-8, clipboard target `c`, BEL-terminated.
        assert_eq!(s.term, b"\x1b]52;c;aMOpbGxvCndvcmxk\x07");
        assert_eq!(*calls.borrow(), ["héllo\nworld"]);
    }

    #[test]
    fn a_failing_platform_tool_does_not_block_osc_52() {
        let calls = Calls::default();
        let mut s = sink(true, &calls);
        s.copy("hi").unwrap();
        assert_eq!(s.term, b"\x1b]52;c;aGk=\x07");
        assert_eq!(calls.borrow().len(), 1);
    }

    #[test]
    fn a_failing_terminal_write_does_not_block_the_platform_tool() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let calls = Calls::default();
        let rec = calls.clone();
        let mut s = SystemClipboard::with(Broken, move |t: &str| {
            rec.borrow_mut().push(t.into());
            Ok(())
        });
        s.copy("hi").unwrap();
        assert_eq!(*calls.borrow(), ["hi"]);

        // Only when both paths fail is the copy reported as failed.
        let mut s = SystemClipboard::with(Broken, |_: &str| bail!("no pbcopy"));
        let err = s.copy("hi").unwrap_err().to_string();
        assert!(err.contains("no pbcopy"), "{err}");
    }

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
