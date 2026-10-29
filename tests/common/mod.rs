//! Shared fixtures for the CLI integration tests.

// Each integration-test crate compiles this module separately and uses a
// different subset of it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// Everything through which the developer's shell could reach into a test:
/// herdr's session (so identity resolves to the dev's pane, `list` asks the
/// dev's socket for liveness, auto-dock drives the dev's herdr), the agent
/// session, the dev's feed, and the dev's config dir on Linux.
const LEAKY_VARS: &[&str] = &[
    "HERDR_ENV",
    "HERDR_SOCKET_PATH",
    "HERDR_BIN_PATH",
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "FEEDR_AGENT",
    "FEEDR_FEED",
    "CLAUDE_CODE_SESSION_ID",
    "XDG_CONFIG_HOME",
];

/// Hold a `feedr` child to the test's fixture, whatever the parent shell
/// exports: `home` becomes its HOME (and so its config dir and skill install
/// targets), and none of [`LEAKY_VARS`] is inherited. A test that wants one
/// of them sets it explicitly after this call.
pub fn hermetic<'a>(cmd: &'a mut Command, home: &Path) -> &'a mut Command {
    for var in LEAKY_VARS {
        cmd.env_remove(var);
    }
    cmd.env("HOME", home)
}

/// A hermetic `feedr` with `home` as its HOME.
pub fn feedr_in(home: &Path) -> Command {
    let mut cmd = Command::cargo_bin("feedr").unwrap();
    hermetic(&mut cmd, home);
    cmd
}

/// Where `dirs::config_dir()` puts feedr's config under a sandboxed HOME.
pub fn config_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/herdr-feedr")
    } else {
        home.join(".config/herdr-feedr")
    }
}
