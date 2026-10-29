# Research: a throwaway herdr server for live end-to-end tests

Ticket: [#44](https://github.com/Adroz/herdr-feedr/issues/44). Sources: herdr source at tag **v0.9.3** (`7b116c0`; paths below are relative to the herdr repo root), the installed `herdr 0.9.3` binary's offline help and `herdr api schema --json`, and feedr at `7e55aba`. Three live probes on 2026-10-29 ran against **isolated** servers only. Every herdr call in them started from `env -i` and set only temp `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `HERDR_SOCKET_PATH`, and the CLI was never given a pane id it could default. The user's real `~/.config/herdr/plugins.json` had the same mtime before and after.

## TL;DR

- **Feasible, and verified live end to end.** A test can start `herdr server` as a child process with its own socket and config/state dirs, create workspaces and panes over the CLI, link a plugin from a temp path, watch a real feedr sidebar auto-close, and see `server.stop` remove both sockets and every pane process. Nothing touches the user's herdr.
- **`HERDR_SOCKET_PATH` alone is not isolation. It is dangerous.** It moves only the two sockets. `session.json`, `plugins.json`, `config.toml`, logs and plugin dirs still resolve under `$XDG_CONFIG_HOME` or `~/.config/herdr`. A server started with only a socket override would **restore the user's saved session** (re-spawning their workspaces' shells) and **overwrite their `session.json`** on exit. Always set `XDG_CONFIG_HOME` and `XDG_STATE_HOME` too, or a temp `HOME`. Better still, set all of them.
- **Headless works without a client.** A fresh server has zero workspaces and stays that way. herdr creates a default workspace only when a shell client is attached. `workspace create`, `pane split`, `plugin link` and `plugin pane open --target-pane` all work headless.
- **`server.stop` is a full teardown:** HUP→TERM→KILL to each pane's whole process session, both socket files removed, process exits 0. The CLI `herdr server stop` waits for both sockets to disappear.
- **Two gotchas for the test:** macOS's 104-byte `sun_path` limit, which rules out the long scratchpad path for the socket. And a freshly opened sidebar does not arm until it sees a `layout.updated` *after* it subscribed (side finding below).

## 1. Socket path, config and state dirs; coexistence

**API socket.** `session::active_api_socket_path` (`src/session.rs:173-181`) resolves it in this order:

1. an explicit `--session <name>` gives `<config_dir>/sessions/<name>/herdr.sock`;
2. otherwise `HERDR_SOCKET_PATH` (`src/api/mod.rs:20`);
3. otherwise `<config_dir>[/sessions/$HERDR_SESSION]/herdr.sock` (`session.rs:161-171`).

The server binds the same `api::socket_path()` (`src/api/mod.rs:91-93`, `src/server/headless/bootstrap.rs:35-47`), and every CLI subcommand's `ApiClient::local()` connects to it too (`src/api/client.rs:24,38-40`).

**Client socket.** `HERDR_SOCKET_PATH` overrides the client (TUI) socket as well. It is derived from the API path by inserting `-client` (`herdr.sock` → `herdr-client.sock`), and it beats the legacy `HERDR_CLIENT_SOCKET_PATH` (`src/server/socket_paths.rs:9-56`).

**Everything else ignores the socket override.** All of these hang off `config::config_dir()`, which is `$XDG_CONFIG_HOME/herdr`, else `$HOME/.config/herdr` (`src/config/io.rs:30-35, 61-68`):

| Path | Source |
|---|---|
| `session.json`, `session-history.json` | `session::data_dir()`; `src/persist/io.rs:10-16` |
| `herdr-server.log` | `session::data_dir()`; `src/logging.rs:12-18` |
| `plugins.json` and its lock | `src/persist/plugin_registry.rs:12,16` |
| `plugins/` (managed checkouts, per-plugin config dirs) | `src/plugin_paths.rs:5-19` |
| `config.toml` | unless `HERDR_CONFIG_PATH` is set; `src/config/io.rs:195-200` |

`config::state_dir()` is `$XDG_STATE_HOME/herdr`, else `$HOME/.local/state/herdr` (`io.rs:37-42, 87-94`). It holds plugin state dirs (`plugin_paths.rs:21-25`), the agent-detection manifest cache (`src/detect/manifest_update.rs:498`), and client preferences.

`session::data_dir()` keys off `HERDR_SESSION`, not the socket (`session.rs:96-101, 157-167`). With a production policy the server restores `session.json` at start and saves it on exit (`src/app/mod.rs:78-84, 373`; `src/server/headless.rs:670-673`). It also loads `plugins.json` at start (`app/mod.rs:175-181, 517`). Hence the TL;DR warning.

**`--session <name>` is not enough either.** It moves the sockets, `session.json` and logs into `sessions/<name>/`. `plugins.json`, `plugins/`, `config.toml` and the state dir stay shared with the default session (same table).

**Debug builds** use `herdr-dev` instead of `herdr` for all of these dir names (`io.rs:22-28`). This is irrelevant to the installed release binary.

**Coexistence: yes.** Binding runs `prepare_socket_path`, which removes a *stale* socket file but rejects a live one with `AddrInUse` (`socket_paths.rs:59-70`; tests at `:119-170`). The server then exits with "already running" (`bootstrap.rs:39-44`). Two servers with different socket paths never contend. The macOS "persistent user service context" switch only happens when the daemon launcher sets `HERDR_MACOS_SERVER_CONTEXT=user` (`src/platform/macos/bootstrap.rs:23-49`). A directly spawned `herdr server` stays an ordinary child of the test.

**Precedent:** herdr's own integration tests do this. `tests/server_headless.rs:101-133` spawns `herdr server` with `XDG_CONFIG_HOME`, `HERDR_SOCKET_PATH`, `SHELL=/bin/sh`, removes `HERDR_ENV` and `HERDR_CLIENT_SOCKET_PATH`, writes `onboarding = false`, and uses short `/tmp/herdr-server-test-*` dirs (`:22-31`). They do *not* override `XDG_STATE_HOME`, so they write plugin and agent-detection state into the real `~/.local/state/herdr`. feedr's test should also set that variable.

**Verified live (probe 1):** with all four variables set, the server printed the temp socket paths. `session.json`, `plugins.json`, `.plugins.lock`, `plugins/config/feedr-e2e`, the log and `release-notes.json` all landed in the temp `config/herdr/`, and agent-detection and plugin state landed in the temp `state/herdr/`.

## 2. Headless operation and building a layout

- `herdr server` runs the headless server in the foreground. `cli::maybe_run` returns `NotCli` for a bare `server` (`src/cli.rs:111-116`, `src/cli/server.rs:3-25`), and `main` then calls `server::headless::run_server()` (`src/main.rs:560-562`). It prints the socket paths to stderr (`bootstrap.rs:211-222`).
- **No client needed.** The server seeds a workspace only from `HERDR_STARTUP_CWD`, which only the auto-launching client sets (`bootstrap.rs:98-126`). It creates a default workspace only `if latest_shell_client(...).is_some()` (`src/server/headless.rs:499`). Headless, the state starts empty, and it stays empty after the last pane closes. That is what lets the sidebar's "nothing left anywhere → `server.stop`" path run. Panes still get PTYs, sized from `[server] headless_cols/rows` (default 120×40; `src/main.rs:224-227`, `src/config.rs` `DEFAULT_HEADLESS_COLS/ROWS`). Live, panes reported `viewport_rows: 40`.
- **Building a layout:** every one of these is a plain socket request (`herdr api schema --json`) with a CLI wrapper:

  | Step | Socket request | CLI |
  |---|---|---|
  | create a workspace | `workspace.create {cwd, label, env, focus}` | `herdr workspace create --cwd … --label …` |
  | split a pane | `pane.split {target_pane_id, direction, …}` | `herdr pane split <id> --direction right` |
  | close a pane | `pane.close {pane_id}` | `herdr pane close <id>` |
  | list panes | `pane.list` | `herdr pane list` |
  | run a command in a pane | | `herdr pane run <id> <cmd…>` |
  | open a plugin pane | `plugin.pane.open {plugin_id, entrypoint, placement, target_pane_id, direction, env, cwd}` | `herdr plugin pane open --plugin … --entrypoint … --placement split --target-pane … --direction right` |

  `workspace.create` returns `root_pane.pane_id`. A split plugin pane needs a target, given explicitly or taken from the focused pane (`src/app/api/plugins/panes.rs:82-104`).
- **Always pass ids explicitly.** With no id, pane CLI commands fall back to `$HERDR_PANE_ID` (`src/cli/target.rs:203-210`). Inside a herdr pane, that is the user's pane. Spawning herdr with `env_clear()` removes the risk.
- `herdr api schema --json`, `--help` and subcommand help are local: no socket involved (`src/cli/api.rs:25-36`, `cli.rs:106-108`).
- **Verified live (probes 1-2):** `workspace list` returned `[]` on a fresh server, and two `workspace create` calls gave `w1:p1` and `w2:p1`.

## 3. Linking a plugin from a local path, isolated

- `herdr plugin link <path>` sends `plugin.link {path}`. The server loads `<path>/herdr-plugin.toml`, creates the plugin's config and state dirs, and writes the server's own `plugins.json` (`src/app/api/plugins/mod.rs:68-88`). All of these sit under the server's `XDG_CONFIG_HOME` and `XDG_STATE_HOME` (§1).
- **Trap:** if the socket is unreachable, the CLI silently falls back to **writing the registry offline**. It writes `plugins.json` under the *CLI's* config dir (`src/cli/plugin.rs:79-85, 1129-1137, 935-943`). A link run without isolated `XDG_CONFIG_HOME` while the test server is down would edit the user's real registry. Isolate the CLI's environment too, not just the server's.
- `plugin link` does **not** run `[[build]]`. Only `plugin install` does (`cli/plugin.rs:231`). The test therefore should not link the repo root, whose pane command is `./target/release/feedr` run from the plugin root (`panes.rs:329-337`). Instead it writes a temp manifest whose `[[panes]] command` is the absolute `env!("CARGO_BIN_EXE_feedr")`. It also omits `[[events]]`, so `on-tab-created.sh` never fires.
- **What the plugin pane gets:**
  - It inherits the server's environment: portable-pty's `CommandBuilder::new` snapshots `get_base_env()` (`vendor/portable-pty/src/cmdbuilder.rs:252-257`), and herdr never calls `env_clear`.
  - herdr adds `HERDR_SOCKET_PATH` (the *server's* socket), `HERDR_ENV`, `HERDR_PLUGIN_*` and `HERDR_BIN_PATH` (`panes.rs:236-268`), plus `HERDR_WORKSPACE_ID`, `HERDR_TAB_ID` and `HERDR_PANE_ID` (`src/pane.rs:185-197`).
  - The sidebar therefore talks to the test server, and feedr's `socket_path_from_env` reads exactly that variable (`src/tui/socket.rs:204-215`).
  - Because `HOME` is inherited, feedr's `dirs::config_dir()` resolves to `$HOME/Library/Application Support/herdr-feedr` on macOS (dirs-sys 0.5 `home_dir` reads `$HOME` first), so a temp `HOME` isolates feedr's config. `FEEDR_FEED` pins the feed file (`src/cli.rs:219`).
- **Verified live (probes 1-2):** the link wrote only the temp `plugins.json`. `plugin pane open` started the debug `feedr sidebar` beside `w1:p1`, and `pane list` showed it titled `feedr-sidebar`.

## 4. Does `server.stop` tear everything down?

Yes.

1. The API thread sets the stop flag and replies `ok` (`src/api/server.rs:545-553`).
2. The main loop drains internal events, then `initiate_shutdown` → `complete_shutdown` runs. That sends `ServerShutdown` to clients, rejects queued requests, drops client connections and removes the client socket (`headless.rs:427-432`; `src/server/headless/lifecycle.rs:303-375`).
3. `session.json` is saved (`headless.rs:670-673`). This lands in the temp dir when isolated.
4. Dropping the API `ServerHandle` removes the API socket if this server still owns it (`api/server.rs:43-58`).
5. Dropping each `PaneRuntime` sends HUP, then TERM, then KILL (250 ms grace each) to **every process in the pane's session** (`src/pane.rs:1558-1572, 1616-1656`; macOS `session_processes` filters by `getsid`, `src/platform/macos.rs:1064-1078`). Grandchildren go too.

The CLI `herdr server stop` (`src/cli/server.rs:27-44`) sends `server.stop` and then waits up to 15 s until **both** sockets stop accepting connections (`src/session.rs:14, 238-296`). It exits 1 with "not running or cannot be reached at <path>" when nothing is there.

**Verified live:**

- **probe 2:** feedr's own `server.stop` made the server exit 0, the socket dir ended up empty, and the shell and sidebar PIDs were gone.
- **probe 3:** a pane ran `sleep 9999`. `herdr server stop` returned 0 in 0.29 s, the server exited 0, both sockets were gone, the shell and the `sleep` grandchild were gone, and a second `stop` exited 1 as expected.

**Network side effect:** with `[update] manifest_check` on (the default), the server shells out to `curl https://herdr.dev/agent-detection/*.toml` at start (seen live in probe 1). That needs network and writes to the state dir. Write `[update]\nversion_check = false\nmanifest_check = false` into the temp `config.toml` (`src/config/model.rs:31-46`). Probe 3 did this and saw no `curl` child.

## 5. Verdict and a gated test sketch

**Feasible.** The auto-close flows were run for real against an isolated server (probe 2):

- **Case 1:** a sidebar sits beside shell `w1:p1`, and `w2:p1` exists elsewhere. Closing `w1:p1` made the sidebar exit within 1.5 s. Tab and workspace `w1` vanished, `w2` was untouched, and the server kept running.
- **Case 2:** the same setup in `w2`, with nothing else anywhere. Closing `w2:p1` made the sidebar call `server.stop`. The server exited 0, both sockets were gone, and no children were left.

Constraints the test must honour:

1. **Socket path under 104 bytes** (macOS `sun_path`). Use a short dir such as `std::env::temp_dir()/hf-<pid>-<n>/herdr.sock`. That is about 75 bytes on macOS, and the derived `herdr-client.sock` must fit too. Assert the length up front.
2. **Spawn every herdr process with `env_clear()`**, then set `PATH`, a temp `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `HERDR_SOCKET_PATH`, `SHELL=/bin/sh`, `TERM` and `FEEDR_FEED`. Never inherit `HERDR_SOCKET_PATH`, `HERDR_PANE_ID` or `HERDR_SESSION`, and never pass `--session`.
3. **A fresh temp root per test**, or `session.json` restore re-spawns the last run's panes.
4. **Arm the sidebar explicitly.** After the sidebar's title shows up, wait briefly for its subscription, then split and close a scratch pane so it sees a `layout.updated` with company (see side finding).
5. **Teardown in `Drop`:** run `herdr server stop` with the isolated environment, then `kill` and `wait` the child PID if it's still alive, then remove the temp dirs. Only ever signal the PID we spawned.

Sketch (`tests/herdr_e2e.rs`, an integration test so `CARGO_BIN_EXE_feedr` exists; gated like `src/tui/clipboard_e2e.rs`):

```rust
//! Live sidebar auto-close against a throwaway herdr server (#44). Needs a
//! `herdr` on PATH; gated by FEEDR_E2E_HERDR=1, passes trivially otherwise.
//! Every herdr process starts from an empty environment pointed at a temp
//! root, so the developer's running herdr is never reachable.
use serde_json::Value;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn enabled() -> bool {
    cfg!(unix) && std::env::var("FEEDR_E2E_HERDR").as_deref() == Ok("1")
}

struct Herdr {
    root: PathBuf,   // HOME / XDG dirs / plugin / feed (any length)
    sock_dir: PathBuf, // short: sun_path is 104 bytes on macOS
    sock: PathBuf,
    bin: PathBuf,
    child: Option<Child>,
}

impl Herdr {
    fn start() -> Herdr {
        let id = format!("{}-{}", std::process::id(), rand_suffix());
        let root = std::env::temp_dir().join(format!("feedr-e2e-{id}"));
        let sock_dir = std::env::temp_dir().join(format!("hf-{id}"));
        let sock = sock_dir.join("herdr.sock");
        assert!(sock_dir.join("herdr-client.sock").as_os_str().len() < 100);
        for d in ["home", "config/herdr", "state", "plugin", "work"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::create_dir_all(&sock_dir).unwrap();
        std::fs::write(root.join("config/herdr/config.toml"),
            "onboarding = false\n[update]\nversion_check = false\nmanifest_check = false\n").unwrap();
        std::fs::write(root.join("work/feed.md"), "# Feed\n\n- [ ] e2e\n").unwrap();
        std::fs::write(root.join("plugin/herdr-plugin.toml"), format!(
            "id = \"feedr-e2e\"\nname = \"feedr-e2e\"\nversion = \"0.0.0\"\n\
             min_herdr_version = \"0.9.0\"\nplatforms = [\"linux\", \"macos\"]\n\n\
             [[panes]]\nid = \"sidebar\"\ntitle = \"feedr\"\nplacement = \"split\"\n\
             command = [{:?}, \"sidebar\"]\n", env!("CARGO_BIN_EXE_feedr"))).unwrap();
        let bin = which("herdr");
        let mut h = Herdr { root, sock_dir, sock, bin, child: None };
        let child = h.cmd().arg("server")
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        h.child = Some(child);
        wait_until(Duration::from_secs(10), || UnixStream::connect(&h.sock).is_ok());
        h
    }

    /// The ONLY way this test builds a herdr command.
    fn cmd(&self) -> Command {
        let mut c = Command::new(&self.bin);
        c.env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("HERDR_SOCKET_PATH", &self.sock)
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .env("FEEDR_FEED", self.root.join("work/feed.md"));
        c
    }

    fn run(&self, args: &[&str]) -> Value {
        let out = self.cmd().args(args).output().unwrap();
        assert!(out.status.success(), "herdr {args:?}: {out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn panes(&self) -> Vec<Value> {
        self.run(&["pane", "list"])["result"]["panes"].as_array().unwrap().clone()
    }

    fn new_workspace(&self) -> String {
        let cwd = self.root.join("work");
        let v = self.run(&["workspace", "create", "--cwd", cwd.to_str().unwrap()]);
        v["result"]["root_pane"]["pane_id"].as_str().unwrap().into()
    }

    /// Open the sidebar beside `shell` and make sure it is armed.
    fn sidebar_beside(&self, shell: &str) -> String {
        let v = self.run(&["plugin", "pane", "open", "--plugin", "feedr-e2e",
            "--entrypoint", "sidebar", "--placement", "split",
            "--target-pane", shell, "--direction", "right"]);
        let id = v["result"]["plugin_pane"]["pane"]["pane_id"].as_str().unwrap().to_string();
        wait_until(Duration::from_secs(10), || self.panes().iter().any(|p|
            p["pane_id"] == id.as_str() && p["terminal_title_stripped"] == "feedr-sidebar"));
        std::thread::sleep(Duration::from_millis(500)); // its events.subscribe lands
        let extra = self.run(&["pane", "split", shell, "--direction", "down"]);
        let extra = extra["result"]["pane"]["pane_id"].as_str().unwrap().to_string();
        self.run(&["pane", "close", &extra]); // layout.updated with company: armed
        id
    }

    fn server_exited(&mut self) -> bool {
        matches!(self.child.as_mut().unwrap().try_wait(), Ok(Some(_)))
    }
}

impl Drop for Herdr {
    fn drop(&mut self) {
        let _ = self.cmd().args(["server", "stop"]).output();
        if let Some(mut c) = self.child.take() {
            if c.try_wait().ok().flatten().is_none() { let _ = c.kill(); }
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(&self.sock_dir);
    }
}

#[test]
fn a_sidebar_left_alone_closes_its_tab_and_herdr_keeps_running() {
    if !enabled() { eprintln!("skipped: set FEEDR_E2E_HERDR=1 to run"); return; }
    let mut h = Herdr::start();
    let shell = h.new_workspace();
    let elsewhere = h.new_workspace();
    h.run(&["plugin", "link", h.root.join("plugin").to_str().unwrap()]);
    let sidebar = h.sidebar_beside(&shell);
    h.run(&["pane", "close", &shell]);
    wait_until(Duration::from_secs(5), || !h.panes().iter().any(|p| p["pane_id"] == sidebar.as_str()));
    let ids: Vec<_> = h.panes().iter().map(|p| p["pane_id"].clone()).collect();
    assert_eq!(ids, vec![Value::from(elsewhere)], "only the other workspace remains");
    assert!(!h.server_exited(), "a pane remains elsewhere, so herdr keeps running");
}

#[test]
fn the_last_sidebar_anywhere_stops_herdr() {
    if !enabled() { eprintln!("skipped: set FEEDR_E2E_HERDR=1 to run"); return; }
    let mut h = Herdr::start();
    let shell = h.new_workspace();
    h.run(&["plugin", "link", h.root.join("plugin").to_str().unwrap()]);
    h.sidebar_beside(&shell);
    h.run(&["pane", "close", &shell]);
    wait_until(Duration::from_secs(10), || h.server_exited());
    assert!(!h.sock.exists() && !h.sock_dir.join("herdr-client.sock").exists());
}

fn wait_until(limit: Duration, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + limit;
    while !ok() {
        assert!(Instant::now() < end, "timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}
// which(): search $PATH (or honour FEEDR_E2E_HERDR_BIN); rand_suffix(): nanos.
```

Notes on the sketch:

- **Pin feedr's own config.** It relies on feedr's defaults `auto_close = true` and `quit_herdr_when_last = true` (`src/config.rs:98-99`). The test could write `$HOME/Library/Application Support/herdr-feedr/config.toml` (macOS) or `$XDG_CONFIG_HOME/herdr-feedr/config.toml` (Linux) to make them explicit.
- **Run it serially.** The two tests can run in parallel safely because each has its own server. Keep them serial anyway (`--test-threads=1`, or a mutex as herdr's tests do at `tests/server_headless.rs:72-77`) to bound load and flakiness from the timing in `sidebar_beside`.
- **Not in CI by default.** CI needs a `herdr` binary. The gate keeps it opt-in, like the clipboard test.

## Side finding: a plainly opened sidebar never arms

Probe 1 opened the sidebar with `plugin pane open` beside a shell and then closed the shell. The sidebar **stayed**.

- herdr emits the open's `layout.updated` inside the `plugin.pane.open` request (`panes.rs:313-315`). That is before the new feedr process has connected and sent `events.subscribe`.
- `AutoClose` arms only from a `layout.updated` that shows company (`src/tui/auto_close.rs:50-82`), and the initial `pane.list` the sidebar makes is not fed to it.
- So a sidebar beside a shell stays unarmed until some later layout change: a split, resize, swap or close.
- In normal use the `--dock` path (swap-walk, resize, fit) probably supplies that event, but it races with the subscription.

Suggested fix (separate ticket): arm from the sidebar's own first successful `pane.list` when it shows company in its tab. Starting alone would still never arm. This is not verified beyond probe 1; the dock path was not tested.

## Claims not verified live

- Linux behaviour: all probes ran on macOS. The `XDG_*` and `sun_path` (108 bytes on Linux) handling is from source.
- Whether two isolated servers run at once without trouble: from source (§1), not run.
- The sketch itself has not been compiled. The CLI calls and JSON shapes it uses (`result.root_pane.pane_id`, `result.pane.pane_id`, `result.plugin_pane.pane.pane_id`, `result.panes[]`, `terminal_title_stripped`) are the ones the probes printed.
