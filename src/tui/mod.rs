pub mod app;
pub mod input;
pub mod modal;
pub mod socket;
#[cfg(test)]
pub mod test_util;
pub mod view;
pub mod watch;

use crate::config::SidebarConfig;
use anyhow::{bail, Result};
use app::App;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use crossterm::tty::IsTty;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// Events pushed by background threads (file watcher: Task 11; socket
/// subscriber: Task 12).
pub enum AppEvent {
    FeedChanged,
    Agents(Vec<socket::AgentInfo>),
    SocketDown,
}

/// Carried-forward acceptance criterion (Tasks 1-2 review): a feed that
/// exists but can't be read (bad permissions, invalid UTF-8, ...) must fail
/// loudly — never render as a silent, empty sidebar. Missing file is fine
/// (it just means no items yet); every other read error bails before the
/// terminal is ever touched.
fn check_feed_readable(path: &Path) -> Result<()> {
    match std::fs::read_to_string(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => bail!("cannot read {}: {e}", path.display()),
    }
}

pub fn run(feed_path: PathBuf, cfg: SidebarConfig) -> Result<()> {
    // Carried-forward acceptance criterion (Tasks 1-2 review): outside a tty
    // this must be a clean error, not ratatui's internal panic — so check
    // before ratatui::init() ever runs.
    if !std::io::stdout().is_tty() {
        bail!("feedr sidebar requires an interactive terminal");
    }
    check_feed_readable(&feed_path)?;

    let (tx, rx) = mpsc::channel::<AppEvent>();
    let _watcher = watch::spawn(&feed_path, tx.clone())
        .map_err(|e| anyhow::anyhow!("cannot watch {}: {e}", feed_path.display()))?;
    socket::spawn_event_thread(socket::socket_path_from_env(), tx.clone());
    let mut app = App::new(
        feed_path,
        cfg,
        Box::new(socket::UnixSocketClient::from_env()),
    );
    app.reload();

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    let res = event_loop(&mut terminal, &mut app, &rx);
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}

/// Run $EDITOR (may carry args, e.g. "code -w") on the feed file.
pub fn spawn_editor(editor: &str, path: &Path) -> Result<()> {
    let mut parts = editor.split_whitespace();
    let bin = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("$EDITOR is empty"))?;
    let status = std::process::Command::new(bin)
        .args(parts)
        .arg(path)
        .status()?;
    if !status.success() {
        anyhow::bail!("editor exited with {status}");
    }
    Ok(())
}

/// Drain every queued event without blocking. `FeedChanged` is coalesced —
/// a burst (e.g. an atomic rename plus a watcher dedup miss) sets a flag
/// instead of reloading per event; the caller reloads at most once per tick,
/// after the drain. Every other event kind is still applied immediately, in
/// order, via `App::on_event`.
fn drain_events(app: &mut App, rx: &mpsc::Receiver<AppEvent>) -> bool {
    let mut feed_changed = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, AppEvent::FeedChanged) {
            feed_changed = true;
        } else {
            app.on_event(ev);
        }
    }
    feed_changed
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mpsc::Receiver<AppEvent>,
) -> Result<()> {
    loop {
        terminal.draw(|f| view::draw(f, app))?;
        if app.should_quit {
            return Ok(());
        }
        if drain_events(app, rx) {
            app.reload();
        }
        if let Some(path) = app.take_editor_request() {
            let editor = app.editor_cmd.clone().unwrap_or_default();
            ratatui::restore();
            let result = spawn_editor(&editor, &path);
            *terminal = ratatui::init();
            let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
            if let Err(e) = result {
                app.status_msg = Some(format!("editor: {e}"));
            }
            app.reload(); // editor's save wins (spec §6, accepted for v1)
        }
        if event::poll(Duration::from_millis(100))? {
            let ev = event::read()?;
            if app.modal_active() {
                app.handle_modal_event(ev);
            } else {
                let size = terminal.size()?;
                if let Some(action) = input::translate(&ev, app, (size.width, size.height)) {
                    app.apply(action);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_feed_readable_treats_missing_file_as_ok() {
        let dir = tempfile::tempdir().unwrap();
        assert!(check_feed_readable(&dir.path().join("nope.md")).is_ok());
    }

    #[test]
    fn check_feed_readable_bails_on_valid_utf8_present_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.md");
        std::fs::write(&path, "- [ ] A\n").unwrap();
        assert!(check_feed_readable(&path).is_ok());
    }

    #[test]
    fn check_feed_readable_bails_loudly_on_invalid_utf8() {
        // Matches tests/cli.rs's unreadable_feed_errors_instead_of_clobbering:
        // invalid UTF-8 fails the same way permission-denied would, without
        // depending on how the test process's uid interacts with chmod.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.md");
        std::fs::write(&path, [0xFF, 0xFE, 0x00, 0x41]).unwrap();
        let err = check_feed_readable(&path).unwrap_err();
        assert!(err.to_string().contains("cannot read"), "got: {err}");
    }

    #[test]
    fn spawn_editor_runs_the_command_with_args() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("feed.md");
        std::fs::write(&target, "- [ ] A\n").unwrap();
        let script = dir.path().join("fake-editor.sh");
        std::fs::write(&script, "#!/bin/sh\necho \"- [ ] From editor\" >> \"$1\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        // $EDITOR values may carry args ("code -w"); split on whitespace.
        super::spawn_editor(&format!("{} ", script.display()), &target).unwrap();
        assert!(std::fs::read_to_string(&target)
            .unwrap()
            .contains("From editor"));
        assert!(super::spawn_editor("/nonexistent-editor-binary", &target).is_err());
    }

    #[test]
    fn drain_events_coalesces_feed_changed_and_applies_others_immediately() {
        use crate::config::{Side, SidebarConfig};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.md");
        std::fs::write(&path, "- [ ] A\n").unwrap();
        let cfg = SidebarConfig {
            side: Side::Left,
            width: 0.18,
            auto_dock: false,
        };
        let mut app = App::new(path.clone(), cfg, Box::new(socket::FakeHerdr::default()));
        app.reload();
        let initial_rows = app.rows.len();

        let (tx, rx) = mpsc::channel();
        tx.send(AppEvent::FeedChanged).unwrap();
        tx.send(AppEvent::Agents(vec![socket::AgentInfo {
            pane_id: "w1:p1".into(),
            kind: "claude".into(),
            session_id: "abc".into(),
            status: socket::AgentStatus::Working,
        }]))
        .unwrap();
        tx.send(AppEvent::FeedChanged).unwrap();
        tx.send(AppEvent::FeedChanged).unwrap();

        // The burst these three FeedChanged events describe: the file
        // changed on disk while they queued up.
        std::fs::write(&path, "- [ ] A\n- [ ] B\n").unwrap();

        let changed = drain_events(&mut app, &rx);
        assert!(changed, "a queued FeedChanged must be reported");
        // Non-FeedChanged events are applied immediately during the drain.
        assert!(app.statuses.contains_key("abc"));
        // Draining alone must never reload: three coalesced FeedChanged
        // events collapse into one flag, not three reloads (or even one
        // reload before the caller decides to).
        assert_eq!(
            app.rows.len(),
            initial_rows,
            "drain_events must not reload on its own"
        );

        app.reload();
        assert_eq!(
            app.rows.len(),
            initial_rows + 1,
            "the caller's single post-drain reload picks up the whole burst"
        );
    }
}
