//! Real system-clipboard round-trip (#40, coverage inventory #13): copy
//! through the production sink and read it back with `pbpaste`. It touches
//! the developer's clipboard, so it is gated: it runs only on macOS with
//! `FEEDR_E2E_CLIPBOARD=1`, and passes trivially everywhere else (CI).
//! The clipboard's previous text is put back afterwards.

use crate::tui::clipboard::{copy, ClipboardSink, SystemClipboard};
use std::process::Command;

fn enabled() -> bool {
    cfg!(target_os = "macos") && std::env::var("FEEDR_E2E_CLIPBOARD").as_deref() == Ok("1")
}

fn pbpaste() -> String {
    let out = Command::new("pbpaste").output().expect("run pbpaste");
    assert!(out.status.success(), "pbpaste failed: {out:?}");
    String::from_utf8(out.stdout).expect("clipboard holds UTF-8")
}

/// Puts the clipboard's text back when dropped, pass or fail.
struct Restore(String);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = copy(&self.0);
    }
}

#[test]
fn the_production_sink_round_trips_through_the_system_clipboard() {
    if !enabled() {
        eprintln!("skipped: set FEEDR_E2E_CLIPBOARD=1 on macOS to run");
        return;
    }
    let _restore = Restore(pbpaste());

    let mut sink = SystemClipboard::with(Vec::new(), copy);
    for text in [
        "plain ascii".to_string(),
        "héllo wörld — 日本語 🙂\nsecond line\n".to_string(),
        "x".repeat(100 * 1024),
    ] {
        sink.copy(&text).expect("copy");
        assert_eq!(pbpaste(), text, "round-trip of {} bytes", text.len());
    }
    assert!(
        sink.term.starts_with(b"\x1b]52;c;"),
        "the OSC 52 path ran alongside the tool"
    );
}
