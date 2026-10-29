//! The feed write lock (SPEC §6).
//!
//! Every read-modify-write of a feed holds an exclusive advisory `flock` on
//! a sibling `<feed>.lock` from before the read until after the atomic
//! rename. The lock lives on its own file, not the feed, because
//! `save_atomic` replaces the feed's inode: a lock on the feed itself would
//! be on a file that no longer has the feed's name once anyone writes.
//!
//! The lock file is created on demand and never removed — removing it would
//! let a waiter lock an unlinked inode while a newcomer locks a fresh one.
//! `flock` is released by the kernel when the holder exits, so a crashed
//! writer can't leave the feed locked.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a CLI writer waits for the lock before giving up.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the sidebar waits. It can't redraw while waiting, so it gives
/// up fast, reports the feed busy, and the user simply retries; the
/// environment override does not apply to it.
pub const SIDEBAR_TIMEOUT: Duration = Duration::from_millis(250);

/// Overrides the CLI's [`DEFAULT_TIMEOUT`], in milliseconds.
pub const TIMEOUT_ENV: &str = "FEEDR_LOCK_TIMEOUT_MS";

const POLL: Duration = Duration::from_millis(10);

/// The CLI's write-lock timeout: [`TIMEOUT_ENV`] when it's a valid number
/// of milliseconds, else [`DEFAULT_TIMEOUT`].
pub fn timeout_from_env() -> Duration {
    std::env::var(TIMEOUT_ENV)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// `feed.md` → `feed.md.lock`, in the same directory.
pub fn lock_path(feed: &Path) -> PathBuf {
    let mut name = feed.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    feed.with_file_name(name)
}

/// Held write lock; dropping it releases the lock.
#[derive(Debug)]
pub struct FeedLock {
    _file: File,
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error(
        "timed out after {timeout:?} waiting for the feed lock {} (another feedr is writing)",
        path.display()
    )]
    Timeout { path: PathBuf, timeout: Duration },
    #[error("cannot lock {}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Take the exclusive write lock for `feed`, waiting up to `timeout`.
pub fn acquire(feed: &Path, timeout: Duration) -> Result<FeedLock, LockError> {
    let path = lock_path(feed);
    let io = |source| LockError::Io {
        path: path.clone(),
        source,
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(io)?;
    let start = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(FeedLock { _file: file }),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(e)) => return Err(io(e)),
        }
        let waited = start.elapsed();
        if waited >= timeout {
            return Err(LockError::Timeout { path, timeout });
        }
        std::thread::sleep(POLL.min(timeout - waited));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_file_is_a_sibling_of_the_feed() {
        assert_eq!(
            lock_path(Path::new("/a/b/feed.md")),
            Path::new("/a/b/feed.md.lock")
        );
    }

    #[test]
    fn a_second_writer_waits_until_the_first_releases() {
        let dir = tempfile::tempdir().unwrap();
        let feed = dir.path().join("feed.md");
        let held = acquire(&feed, Duration::ZERO).unwrap();
        let waiter = {
            let feed = feed.clone();
            std::thread::spawn(move || acquire(&feed, Duration::from_secs(5)).map(|_| ()))
        };
        std::thread::sleep(Duration::from_millis(50));
        drop(held);
        waiter.join().unwrap().unwrap();
    }
}
