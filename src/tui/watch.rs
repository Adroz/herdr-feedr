use crate::tui::AppEvent;
use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc;

/// Watch the feed for external changes. Returns the watcher — the caller must
/// keep it alive (dropping it stops the watch). Watches the parent directory,
/// not the file: save_atomic replaces the file via rename, which kills
/// per-file watches on most platforms.
pub fn spawn(feed_path: &Path, tx: mpsc::Sender<AppEvent>) -> Result<RecommendedWatcher> {
    let dir = feed_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    std::fs::create_dir_all(&dir)?;
    let file_name = feed_path
        .file_name()
        .context("feed path has no file name")?
        .to_os_string();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if ev
                .paths
                .iter()
                .any(|p| p.file_name() == Some(file_name.as_os_str()))
            {
                // Receiver gone = app quit; nothing to do.
                let _ = tx.send(AppEvent::FeedChanged);
            }
        }
    })?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn watcher_reports_atomic_save() {
        let dir = tempfile::tempdir().unwrap();
        let feed = dir.path().join("feed.md");
        std::fs::write(&feed, "- [ ] A\n").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let _watcher = spawn(&feed, tx).unwrap();
        std::thread::sleep(Duration::from_millis(300)); // let the watch settle
                                                        // The write path the whole app uses — temp file + rename:
        crate::feed::write::save_atomic(&crate::feed::parse::parse("- [x] A\n"), &feed).unwrap();
        let ev = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("expected a FeedChanged event");
        assert!(matches!(ev, crate::tui::AppEvent::FeedChanged));
    }

    #[test]
    fn watcher_ignores_sibling_files() {
        let dir = tempfile::tempdir().unwrap();
        let feed = dir.path().join("feed.md");
        std::fs::write(&feed, "- [ ] A\n").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let _watcher = spawn(&feed, tx).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        // macOS FSEvents replays events that predate the watch — the tempdir
        // creation and the initial feed.md write above both arrive here, and
        // both legitimately match. Drain them so the assertion judges only
        // what the sibling write produces.
        while rx.try_recv().is_ok() {}
        std::fs::write(dir.path().join("other.txt"), "noise").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_err());
    }
}
