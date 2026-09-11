pub mod app;
pub mod input;
pub mod modal;
pub mod socket;
#[cfg(test)]
pub mod test_util;
pub mod view;

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

    let (_tx, rx) = mpsc::channel::<AppEvent>();
    let mut app = App::new(feed_path, cfg, Box::new(NoHerdr));
    app.reload();

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    let res = event_loop(&mut terminal, &mut app, &rx);
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}

/// Placeholder herdr client until the live one lands (Task 12): every call
/// fails with the message the degraded UX shows. (`open_resume_tab` is the
/// trait's provided method — it fails via `create_tab` here.)
struct NoHerdr;
impl socket::Herdr for NoHerdr {
    fn list_agents(&mut self) -> Result<Vec<socket::AgentInfo>> {
        anyhow::bail!("herdr socket unavailable")
    }
    fn focus_agent(&mut self, _target: &str) -> Result<()> {
        anyhow::bail!("herdr socket unavailable")
    }
    fn focus_pane(&mut self, _pane_id: &str) -> Result<()> {
        anyhow::bail!("herdr socket unavailable")
    }
    fn create_tab(&mut self, _label: &str) -> Result<String> {
        anyhow::bail!("herdr socket unavailable")
    }
    fn agent_start(
        &mut self,
        _name: &str,
        _kind: &str,
        _pane_id: &str,
        _args: &[String],
    ) -> Result<()> {
        anyhow::bail!("herdr socket unavailable")
    }
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
        while let Ok(ev) = rx.try_recv() {
            app.on_event(ev);
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
}
