//! Live sidebar auto-close against a throwaway herdr server (#47; research:
//! `docs/research/herdr-test-server.md`). Needs `herdr` on PATH (or
//! `FEEDR_E2E_HERDR_BIN`); gated by `FEEDR_E2E_HERDR=1`, passes trivially
//! otherwise (CI).
//!
//! Every herdr process starts from an empty environment pointed at a temp
//! root: its own socket, config, state and HOME. Setting only
//! `HERDR_SOCKET_PATH` would still restore and overwrite the developer's
//! session and plugin registry. Pane ids are always passed explicitly, since
//! the CLI otherwise falls back to `$HERDR_PANE_ID`.

use serde_json::Value;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn enabled() -> bool {
    std::env::var("FEEDR_E2E_HERDR").as_deref() == Ok("1")
}

/// One server at a time: the tests are timing-sensitive, so bound the load.
static SERIAL: Mutex<()> = Mutex::new(());

const SIDEBAR_TITLE: &str = "feedr-sidebar";

struct Herdr {
    /// HOME, XDG dirs, the plugin and the feed. Any length.
    root: tempfile::TempDir,
    /// Holds the sockets: `sun_path` is 104 bytes on macOS, so keep it short.
    sock_dir: tempfile::TempDir,
    bin: PathBuf,
    child: Option<Child>,
}

impl Herdr {
    /// Start an isolated server with a feedr sidebar plugin linked.
    /// `feedr_config` is the body of feedr's own `config.toml`.
    fn start(feedr_config: &str) -> Herdr {
        let root = tempfile::tempdir().unwrap();
        let sock_dir = tempfile::Builder::new().prefix("hf-").tempdir().unwrap();
        assert!(
            sock_dir.path().join("herdr-client.sock").as_os_str().len() < 100,
            "socket path too long for sun_path"
        );
        let r = root.path();
        for d in ["home", "config/herdr", "state", "plugin", "work"] {
            std::fs::create_dir_all(r.join(d)).unwrap();
        }
        std::fs::write(
            r.join("config/herdr/config.toml"),
            "onboarding = false\n[update]\nversion_check = false\nmanifest_check = false\n",
        )
        .unwrap();
        // feedr's config dir: dirs::config_dir() under the temp HOME (macOS)
        // or XDG_CONFIG_HOME (Linux). Writing both covers either.
        for dir in [
            r.join("home/Library/Application Support/herdr-feedr"),
            r.join("config/herdr-feedr"),
        ] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("config.toml"), feedr_config).unwrap();
        }
        std::fs::write(r.join("work/feed.md"), "# Feed\n\n- [ ] e2e\n").unwrap();
        // No [[build]] (`plugin link` doesn't run it) and no [[events]], so
        // auto-dock never fires: each test opens exactly the sidebars it wants.
        std::fs::write(
            r.join("plugin/herdr-plugin.toml"),
            format!(
                "id = \"feedr-e2e\"\nname = \"feedr-e2e\"\nversion = \"0.0.0\"\n\
                 min_herdr_version = \"0.9.0\"\nplatforms = [\"linux\", \"macos\"]\n\n\
                 [[panes]]\nid = \"sidebar\"\ntitle = \"feedr\"\nplacement = \"split\"\n\
                 command = [{:?}, \"sidebar\"]\n",
                env!("CARGO_BIN_EXE_feedr")
            ),
        )
        .unwrap();

        let mut h = Herdr {
            root,
            sock_dir,
            bin: herdr_bin(),
            child: None,
        };
        let child = h
            .cmd()
            .arg("server")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn herdr server");
        h.child = Some(child);
        let sock = h.sock();
        wait_until("herdr's socket", Duration::from_secs(10), || {
            UnixStream::connect(&sock).is_ok()
        });
        let plugin = h.root.path().join("plugin");
        h.run(&["plugin", "link", plugin.to_str().unwrap()]);
        h
    }

    fn sock(&self) -> PathBuf {
        self.sock_dir.path().join("herdr.sock")
    }

    /// The ONLY way these tests build a herdr command.
    fn cmd(&self) -> Command {
        let r = self.root.path();
        let herdr_dir = self.bin.parent().unwrap().to_str().unwrap().to_string();
        let mut c = Command::new(&self.bin);
        c.env_clear()
            .env("PATH", format!("{herdr_dir}:/usr/bin:/bin:/usr/sbin:/sbin"))
            .env("HOME", r.join("home"))
            .env("XDG_CONFIG_HOME", r.join("config"))
            .env("XDG_STATE_HOME", r.join("state"))
            .env("HERDR_SOCKET_PATH", self.sock())
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .env("FEEDR_FEED", r.join("work/feed.md"));
        c
    }

    fn run(&self, args: &[&str]) -> Value {
        let out = self.cmd().args(args).output().unwrap();
        assert!(out.status.success(), "herdr {args:?}: {out:?}");
        if out.stdout.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("herdr {args:?} printed non-JSON ({e}): {out:?}"))
    }

    fn panes(&self) -> Vec<Value> {
        self.run(&["pane", "list"])["result"]["panes"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn pane_ids(&self) -> Vec<String> {
        self.panes()
            .iter()
            .map(|p| p["pane_id"].as_str().unwrap().to_string())
            .collect()
    }

    fn has_pane(&self, id: &str) -> bool {
        self.pane_ids().iter().any(|p| p == id)
    }

    /// A new workspace; returns its root shell's pane id.
    fn new_workspace(&self) -> String {
        let cwd = self.root.path().join("work");
        let v = self.run(&["workspace", "create", "--cwd", cwd.to_str().unwrap()]);
        v["result"]["root_pane"]["pane_id"].as_str().unwrap().into()
    }

    /// Open the sidebar beside `shell` with a plain `plugin pane open` (no
    /// dock, no later layout change), then let it start up.
    fn sidebar_beside(&self, shell: &str) -> String {
        let v = self.run(&[
            "plugin",
            "pane",
            "open",
            "--plugin",
            "feedr-e2e",
            "--entrypoint",
            "sidebar",
            "--placement",
            "split",
            "--target-pane",
            shell,
            "--direction",
            "right",
            "--no-focus",
        ]);
        let id = v["result"]["plugin_pane"]["pane"]["pane_id"]
            .as_str()
            .unwrap()
            .to_string();
        wait_until("the sidebar's title", Duration::from_secs(10), || {
            self.panes().iter().any(|p| {
                p["pane_id"] == id.as_str() && p["terminal_title_stripped"] == SIDEBAR_TITLE
            })
        });
        // The title is set before the event loop; its first tick (the
        // startup pane.list that arms it) follows within a loop step.
        std::thread::sleep(Duration::from_millis(500));
        id
    }

    /// Make `pane` an agent pane, as an agent's hook integration would.
    fn report_agent(&self, pane: &str) {
        self.run(&[
            "pane",
            "report-agent",
            "--source",
            "e2e",
            "--agent",
            "claude",
            "--state",
            "idle",
            pane,
        ]);
        wait_until("the agent to be listed", Duration::from_secs(5), || {
            self.run(&["agent", "list"])["result"]["agents"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x["pane_id"] == pane))
        });
    }

    fn close_pane(&self, pane: &str) {
        self.run(&["pane", "close", pane]);
    }

    fn workspace_ids(&self) -> Vec<String> {
        self.run(&["workspace", "list"])["result"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["workspace_id"].as_str().unwrap().to_string())
            .collect()
    }

    /// The shell exits by itself, as `exit` typed at its prompt would.
    fn exit_shell(&self, pane: &str) {
        self.run(&["pane", "run", pane, "exit"]);
    }

    fn server_exited(&mut self) -> bool {
        matches!(self.child.as_mut().unwrap().try_wait(), Ok(Some(_)))
    }
}

impl Drop for Herdr {
    fn drop(&mut self) {
        let _ = self.cmd().args(["server", "stop"]).output();
        if let Some(mut c) = self.child.take() {
            if c.try_wait().ok().flatten().is_none() {
                let _ = c.kill();
            }
            let _ = c.wait();
        }
    }
}

/// `FEEDR_E2E_HERDR_BIN`, else the first `herdr` on PATH.
fn herdr_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("FEEDR_E2E_HERDR_BIN") {
        return p.into();
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join("herdr"))
        .find(|p| p.is_file())
        .expect("herdr not found on PATH (or set FEEDR_E2E_HERDR_BIN)")
}

fn wait_until(what: &str, limit: Duration, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + limit;
    while !ok() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run `test` against a fresh server, or skip when the gate is off.
fn with_herdr(feedr_config: &str, test: impl FnOnce(&mut Herdr)) {
    if !enabled() {
        eprintln!("skipped: set FEEDR_E2E_HERDR=1 to run");
        return;
    }
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = Herdr::start(feedr_config);
    test(&mut h);
}

#[derive(PartialEq)]
enum Shell {
    Plain,
    /// Listed in `agent.list`: closing it changes the agent set.
    Agent,
}

/// A sidebar beside `shell`, which `close` then removes: the sidebar must
/// exit (closing its tab and workspace) while `elsewhere` keeps herdr up.
fn closes_with_its_shell(h: &mut Herdr, kind: Shell, close: impl FnOnce(&Herdr, &str)) {
    let shell = h.new_workspace();
    let elsewhere = h.new_workspace();
    let elsewhere_ws = h.workspace_ids().pop().unwrap();
    let sidebar = h.sidebar_beside(&shell);
    if kind == Shell::Agent {
        h.report_agent(&shell);
    }
    close(h, &shell);
    wait_until("the sidebar to exit", Duration::from_secs(5), || {
        !h.has_pane(&sidebar)
    });
    assert_eq!(
        h.pane_ids(),
        [elsewhere],
        "only the other workspace's pane remains"
    );
    assert_eq!(
        h.workspace_ids(),
        [elsewhere_ws],
        "the sidebar's workspace closed"
    );
    assert!(!h.server_exited(), "a pane remains, so herdr keeps running");
}

/// Also the #46 regression: a plainly opened sidebar sees no layout with
/// company after it subscribes, so it must arm from its startup pane.list.
#[test]
fn a_sidebar_closes_when_its_shell_is_closed() {
    with_herdr("", |h| {
        closes_with_its_shell(h, Shell::Plain, Herdr::close_pane)
    });
}

/// `exit` pushes no pane_closed, unlike `pane close`.
#[test]
fn a_sidebar_closes_when_its_shell_exits() {
    with_herdr("", |h| {
        closes_with_its_shell(h, Shell::Plain, Herdr::exit_shell)
    });
}

/// The agent set changes, so the sidebar's subscriber replaces its stream.
#[test]
fn a_sidebar_closes_when_its_agent_pane_is_closed() {
    with_herdr("", |h| {
        closes_with_its_shell(h, Shell::Agent, Herdr::close_pane)
    });
}

#[test]
fn a_sidebar_closes_when_its_agent_shell_exits() {
    with_herdr("", |h| {
        closes_with_its_shell(h, Shell::Agent, Herdr::exit_shell)
    });
}

#[test]
fn the_last_sidebar_anywhere_stops_herdr() {
    with_herdr("", |h| {
        let shell = h.new_workspace();
        h.sidebar_beside(&shell);
        h.close_pane(&shell);
        wait_until("herdr to stop", Duration::from_secs(10), || {
            h.server_exited()
        });
        let status = h.child.as_mut().unwrap().wait().unwrap();
        assert!(status.success(), "server exited with {status}");
        let socks = h.sock_dir.path();
        assert!(!socks.join("herdr.sock").exists());
        assert!(!socks.join("herdr-client.sock").exists());
    });
}

#[test]
fn auto_close_off_keeps_the_sidebar() {
    with_herdr("[sidebar]\nauto_close = false\n", |h| {
        let shell = h.new_workspace();
        h.new_workspace();
        let sidebar = h.sidebar_beside(&shell);
        h.close_pane(&shell);
        std::thread::sleep(Duration::from_secs(2)); // well past the debounce
        assert!(h.has_pane(&sidebar));
    });
}
