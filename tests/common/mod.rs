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

/// A fake `herdr` executable for script and CLI tests: every invocation
/// appends its argv (space-joined, one line) to a log, and its stdout is the
/// first canned reply whose shell `case` pattern matches that argv — `{}`
/// when none does. A reply of `None` makes that invocation exit 1 instead.
pub struct FakeHerdrBin {
    pub path: PathBuf,
    log: PathBuf,
}

impl FakeHerdrBin {
    pub fn new(dir: &Path, replies: &[(&str, Option<&str>)]) -> Self {
        use std::fmt::Write as _;
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-herdr");
        let log = dir.join("fake-herdr.log");
        std::fs::write(&log, "").unwrap();
        let mut arms = String::new();
        for (i, (pattern, reply)) in replies.iter().enumerate() {
            let body = match reply {
                Some(text) => {
                    let file = dir.join(format!("fake-herdr-reply-{i}"));
                    std::fs::write(&file, text).unwrap();
                    format!("cat '{}'", file.display())
                }
                None => "exit 1".to_string(),
            };
            writeln!(arms, "  {pattern}) {body} ;;").unwrap();
        }
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\ncase \"$*\" in\n{arms}  *) echo '{{}}' ;;\nesac\n",
            log = log.display()
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        FakeHerdrBin { path, log }
    }

    /// Every recorded invocation's argv, in order.
    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// A minimal herdr socket: answers every NDJSON request with an empty
/// result and records `method params` per request. Enough for the dock
/// paths that reach herdr over the socket rather than the CLI.
pub struct FakeHerdrSocket {
    pub path: PathBuf,
    requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    _dir: tempfile::TempDir,
}

impl FakeHerdrSocket {
    pub fn start() -> Self {
        use std::io::{BufRead, BufReader, Write};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let Ok(req) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                log.lock().unwrap().push(format!(
                    "{} {}",
                    req["method"].as_str().unwrap_or(""),
                    req["params"]
                ));
                let reply = serde_json::json!({"id": req["id"], "result": {}});
                let _ = stream.write_all(format!("{reply}\n").as_bytes());
            }
        });
        FakeHerdrSocket {
            path,
            requests,
            _dir: dir,
        }
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}
