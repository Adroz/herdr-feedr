# feedr Sidebar TUI Implementation Plan (Plan 2 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `feedr sidebar` — a ratatui TUI in the same binary that renders the feed with live agent-status glyphs from the herdr socket, implements every spec §3 interaction, refreshes automatically on file changes, and ships an idempotent `feedr sidebar --dock` launcher using the herdr-beads dock pattern.

**Architecture:** Single-threaded ratatui event loop; background threads (file watcher, socket subscriber) push `AppEvent`s over an mpsc channel. Every mutation reads the feed fresh, applies one change through the Plan 1 `ops` functions with `Authority::Human`, and writes with `save_atomic` — no new write paths. Items are addressed by a `(title, state)` key that is re-located against the fresh read on every action, so concurrent edits by agents never corrupt anything (a stale action degrades to a status-line message). All herdr access goes through a `Herdr` trait so tests use a fake; the real client speaks NDJSON to the unix socket.

**Tech Stack:** Rust (edition 2021, same crate), ratatui 0.29 + crossterm 0.28 (TUI), tui-textarea 0.7 (modal text editing), notify 8 (file watch), serde_json 1 (NDJSON socket protocol).

**Spec:** `docs/SPEC.md` — §3 (sidebar), §7 (socket plumbing), §6 (concurrency), §2/§2a via the existing ops. Research: `docs/research/agent-task-visibility.md` (socket protocol, on branch `research/agent-task-visibility`) and `docs/research/pane-placements.md` (dock pattern, on branch `research/pane-placements`). Plan 1 (`docs/superpowers/plans/2026-09-11-feedr-core-cli.md`) built the model/ops/CLI this plan calls. Plan 3 (packaging: `herdr-plugin.toml`, SKILL.md, registry, `toggle-feedr` action, `tab.created` auto-dock hook) is **not** this plan.

**Testing strategy (explicit):**
- **Rendering:** ratatui `TestBackend` — draw into an in-memory buffer, assert on extracted row strings (`src/tui/test_util.rs`).
- **Input:** `input::translate` is a pure function `(event, app-state, size) → Option<Action>`; unit-tested directly, including mouse hit-testing.
- **Mutations:** `App::apply` tested against real temp feed files, asserting the file contents after each action (this is the concurrency-critical path: fresh read → op → atomic save).
- **Socket:** protocol parsing unit-tested against the payload shapes verified in the research doc; app logic uses a `FakeHerdr`; the real `UnixSocketClient` gets one cheap integration test against an in-process `UnixListener` fake server.
- **Dock:** herdr CLI calls go through a `Runner` trait; tests use a fake runner and assert the exact command sequences.
- **No terminal e2e:** driving a real pty is expensive and flaky; the `assert_cmd` tests only cover CLI wiring (`sidebar --help`).

---

## Decisions (and why)

1. **File-watch: `notify` (event-driven), watching the feed's parent directory, not the file.** `save_atomic` replaces the file via temp-file + rename, which kills a per-file watch on most platforms; a non-recursive directory watch filtered to the feed's filename survives replacement. Chosen over polling because the sidebar should repaint promptly when an agent claims an item, and notify is the ecosystem-standard, well-maintained crate.
2. **`tui-textarea` for the edit modal.** The spec's edit modal needs real multi-line editing (cursor movement, insert/delete anywhere) for title + body. Hand-rolling that is ~150 lines of fiddly, bug-prone code — exactly the wheel `tui-textarea` (MIT, maintained, built for ratatui) already provides. Flagged for the lead since the task brief enumerated only ratatui/crossterm/notify.
3. **Item identity = `(title, state)` key, re-located per action.** Node indices are unstable across concurrent external edits. Exact title+state match against the fresh read is deterministic and testable; a miss means the item changed under us → drop the action with a status message (never guess).
4. **Glyphs (all single-cell, terminal-palette only per spec §8):** item states `·` open, `~` in progress, `?` review, `x` done; agent statuses `>` working, `!` blocked, `✓` done, `-` idle, nothing for unknown/absent. Rail: `«` collapse button, `»` collapsed rail.
5. **Collapse** renders a rail via an app flag and *additionally* best-efforts a herdr `pane resize` (relative amounts only exist in plugin v1, so "restore previous width" is the inverse delta — approximate by design; see research doc). Degrades to render-only outside herdr.
6. **File button** opens the internal read-only modal; `e` inside that modal (or `e` in the main list) opens `$EDITOR`. One click, one behavior; both spec'd behaviors reachable.

**Settled by the lead** (verified against live herdr 0.9.0 CLI + `herdr api schema --json` — executors follow these, no re-litigation):

- **(a) Resume flow:** no herdr method spawns a command. The supported flow is `tab.create` (TabCreateParams: workspace_id/cwd/env/label/focus — spawns a shell pane) then `agent.start` (AgentStartParams: {name, kind, pane_id, args[], timeout_ms}) with `--resume <session-id>` passed through in `args`. CLI: `herdr tab create --label <t> --focus`, then `herdr agent start <name> --kind claude --pane <PANE_ID> -- --resume <id>`. Bonus: `agent.start` succeeding means herdr *detected* the agent, so the item's `@agent` link goes live again with proper status. Implemented in Tasks 3/12; status-line fallback kept for failures.
- **(b) Dock open:** `pane.split` also takes no command (PaneSplitParams: direction/ratio/cwd/env/focus/right_click/target_pane_id/workspace_id). Interim standalone dock = `pane split --direction right` then `pane.send_text` (PaneSendTextParams: {pane_id, text}) typing `exec feedr sidebar` into the new shell; Task 13's one bounded check is the CLI wrapper's send-text subcommand spelling. Plan 3's `plugin pane open` (PluginPaneOpenParams, with placement) replaces this pair.
- **(c)** `tui-textarea` dependency: **approved**.
- **(d)** Collapse via inverse relative resize delta: **accepted** as plugin-v1 reality (no absolute sizing exists).
- **(e)** File button opens the internal viewer; `e` inside it opens `$EDITOR`: **matches spec intent**.
- **(f)** Mouse-first v1: keyboard list-navigation is **deferred to post-v1** (see out-of-scope note in the self-review).

---

## File structure

```
src/
  main.rs          — add `mod tui;`
  cli.rs           — add `Sidebar { --dock }` subcommand (early-dispatch, before feed read)
  config.rs        — extend: [sidebar] table → SidebarConfig { side, width, auto_dock }
  feed/ops.rs      — extend: edit(), remove(), add_in_section() (small new ops, same invariants)
  tui/
    mod.rs         — run(): terminal setup, event loop, $EDITOR suspend/resume; AppEvent enum
    app.rs         — App state, Row model, ItemKey/relocate, Action, apply(), modal state machine
    view.rs        — draw(): toolbar, list, Done(n)/status rows, collapse rail, modals; glyphs
    input.rs       — translate(): key/mouse → Action (pure hit-testing)
    socket.rs      — AgentStatus/AgentInfo, Herdr trait, NDJSON UnixSocketClient, event thread, FakeHerdr
    watch.rs       — notify watcher → AppEvent::FeedChanged
    dock.rs        — Runner trait, HerdrCli, dock() launcher, collapse resize, pane title marker
    test_util.rs   — #[cfg(test)] TestBackend render helper
tests/
  cli.rs           — extend: `sidebar --help` wiring test
```

Tests live inline in `#[cfg(test)]` modules per file (repo convention from Plan 1), except the e2e CLI test.

---

### Task 1: Sidebar config

**Files:**
- Modify: `src/config.rs`

The `[sidebar]` table in the existing `~/.config/herdr-feedr/config.toml`: `side` (`"left"`/`"right"`, default left), `width` (relative resize delta, default `0.18` — the herdr-beads dock value), `auto_dock` (default `false`; parsed here, consumed by Plan 3's `tab.created` hook).

- [ ] **Step 1: Write the failing test** (append inside the existing `tests` module in `src/config.rs`)

```rust
    #[test]
    fn sidebar_config_defaults_and_parse() {
        let dir = tempfile::tempdir().unwrap();
        // Defaults when no config file exists:
        assert_eq!(
            load_sidebar_config(dir.path()),
            SidebarConfig { side: Side::Left, width: 0.18, auto_dock: false }
        );
        // Parsed values:
        std::fs::write(
            dir.path().join("config.toml"),
            "feed_path = \"/tmp/f.md\"\n\n[sidebar]\nside = \"right\"\nwidth = 0.25\nauto_dock = true\n",
        )
        .unwrap();
        assert_eq!(
            load_sidebar_config(dir.path()),
            SidebarConfig { side: Side::Right, width: 0.25, auto_dock: true }
        );
        // Invalid side falls back to left:
        std::fs::write(dir.path().join("config.toml"), "[sidebar]\nside = \"top\"\n").unwrap();
        assert_eq!(load_sidebar_config(dir.path()).side, Side::Left);
        // feed_path resolution still works with [sidebar] present:
        std::fs::write(
            dir.path().join("config.toml"),
            "feed_path = \"/tmp/f.md\"\n[sidebar]\nwidth = 0.2\n",
        )
        .unwrap();
        assert_eq!(resolve_feed_path(None, None, dir.path()), PathBuf::from("/tmp/f.md"));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test config`
Expected: FAIL — `load_sidebar_config`, `SidebarConfig`, `Side` not defined.

- [ ] **Step 3: Implement.** In `src/config.rs`: add the types, extend `FileConfig`, and factor the file read out of `resolve_feed_path` so both paths share it.

Add below the imports:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SidebarConfig {
    pub side: Side,
    /// Relative resize delta used when docking/collapsing (herdr resize
    /// amounts are proportional shares, not columns; 0.18 matches herdr-beads).
    pub width: f64,
    /// Read for Plan 3's tab.created auto-dock hook; the TUI itself ignores it.
    pub auto_dock: bool,
}

#[derive(serde::Deserialize, Default)]
struct SidebarToml {
    side: Option<String>,
    width: Option<f64>,
    auto_dock: Option<bool>,
}
```

Change `FileConfig` to:

```rust
#[derive(Deserialize, Default)]
struct FileConfig {
    feed_path: Option<PathBuf>,
    #[serde(default)]
    sidebar: SidebarToml,
}
```

Replace the body of `resolve_feed_path`'s config-read (the `let cfg: FileConfig = match ...` block) with a call to a shared helper, and add `load_sidebar_config`:

```rust
fn read_file_config(config_dir: &std::path::Path) -> FileConfig {
    match std::fs::read_to_string(config_dir.join("config.toml")) {
        Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
            eprintln!("feedr: warning: ignoring malformed config.toml: {e}");
            FileConfig::default()
        }),
        Err(_) => FileConfig::default(),
    }
}

pub fn load_sidebar_config(config_dir: &std::path::Path) -> SidebarConfig {
    let s = read_file_config(config_dir).sidebar;
    let side = match s.side.as_deref() {
        None | Some("left") => Side::Left,
        Some("right") => Side::Right,
        Some(other) => {
            eprintln!("feedr: warning: sidebar.side \"{other}\" is not left/right; using left");
            Side::Left
        }
    };
    SidebarConfig {
        side,
        width: s.width.unwrap_or(0.18),
        auto_dock: s.auto_dock.unwrap_or(false),
    }
}
```

so `resolve_feed_path` becomes:

```rust
pub fn resolve_feed_path(
    cli_file: Option<PathBuf>,
    env_file: Option<PathBuf>,
    config_dir: &std::path::Path,
) -> PathBuf {
    if let Some(p) = cli_file {
        return p;
    }
    if let Some(p) = env_file {
        return p;
    }
    read_file_config(config_dir)
        .feed_path
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| config_dir.join("feed.md"))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test config`
Expected: PASS — 2 tests (the existing precedence test and the new one).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: [sidebar] config table (side, width, auto_dock)"
```

---

### Task 2: TUI scaffold + `feedr sidebar` subcommand

**Files:**
- Modify: `Cargo.toml`, `src/main.rs`, `src/cli.rs`, `tests/cli.rs`
- Create: `src/tui/mod.rs`

- [ ] **Step 1: Write the failing e2e test** (append to `tests/cli.rs`)

```rust
#[test]
fn sidebar_subcommand_is_wired() {
    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.args(["sidebar", "--help"])
        .assert()
        .success()
        .stdout(contains("sidebar"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test cli sidebar_subcommand_is_wired`
Expected: FAIL — clap rejects the unknown `sidebar` subcommand (exit code 2).

- [ ] **Step 3: Add dependencies to `Cargo.toml`** (append to `[dependencies]`)

```toml
ratatui = "0.29"
crossterm = "0.28"
tui-textarea = "0.7"
notify = "8"
serde_json = "1"
```

(crossterm 0.28 is the version ratatui 0.29 and tui-textarea 0.7 link against — the event types must be one version crate-wide.)

- [ ] **Step 4: Create `src/tui/mod.rs`** — minimal loop, replaced wholesale in Task 7:

```rust
use crate::config::SidebarConfig;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use std::path::PathBuf;
use std::time::Duration;

pub fn run(_feed_path: PathBuf, _cfg: SidebarConfig) -> Result<()> {
    let mut terminal = ratatui::init();
    let res = (|| -> Result<()> {
        loop {
            terminal.draw(|f| {
                f.render_widget(ratatui::text::Text::raw("feedr sidebar — q quits"), f.area())
            })?;
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(k) = event::read()? {
                    if k.code == KeyCode::Char('q') {
                        return Ok(());
                    }
                }
            }
        }
    })();
    ratatui::restore();
    res
}
```

- [ ] **Step 5: Wire the subcommand.** In `src/main.rs` add `mod tui;` under the other `mod` lines. In `src/cli.rs`:

Add to the `Cmd` enum:

```rust
    /// Launch the sidebar TUI in this terminal
    Sidebar,
```

In `run()`, immediately after `path` is resolved (before the `std::fs::read_to_string` block — the TUI does its own reading), add:

```rust
    if matches!(&cli.command, Cmd::Sidebar) {
        let cfg = config::load_sidebar_config(&config::default_config_dir());
        return crate::tui::run(path, cfg);
    }
```

(Match on `&cli.command` — `Cmd` isn't `Copy`, and `cli.command` is consumed by the main `match` below.)

- [ ] **Step 6: Run the full suite**

Run: `cargo test`
Expected: PASS — all existing tests plus `sidebar_subcommand_is_wired`. (First build compiles the new deps; allow a few minutes.)

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs src/cli.rs src/tui tests/cli.rs
git commit -m "feat: scaffold ratatui sidebar behind 'feedr sidebar'"
```

---

### Task 3: Socket protocol types, `Herdr` trait, fake

**Files:**
- Create: `src/tui/socket.rs`
- Modify: `src/tui/mod.rs` (add `pub mod socket;` at the top)

Pure protocol layer first — no I/O yet (the live client is Task 12). Payload shapes come verbatim from `docs/research/agent-task-visibility.md` (branch `research/agent-task-visibility`): `agent.list`/`pane.list` entries carry `pane_id`, `agent`, `agent_status` ∈ `idle|working|blocked|done|unknown`, and `agent_session.value` (the session UUID — the join key to `@agent(kind:id)` tokens in the feed). The envelope *around* the entry array is not pinned by the research doc, so the parser accepts either a bare array or any object containing one array field (tolerant reader).

- [ ] **Step 1: Write the failing tests** (bottom of the new `src/tui/socket.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Entry shape verified live against herdr 0.9.0 in the research doc.
    fn sample_entry() -> serde_json::Value {
        json!({
            "pane_id": "w1:p7", "tab_id": "w1:t5", "workspace_id": "w1",
            "agent": "claude",
            "agent_status": "working",
            "agent_session": {"agent": "claude", "kind": "id", "source": "herdr:claude",
                              "value": "13bb6c2a-1b44-4485-a9c3-e02f5d662dfd"},
            "terminal_title_stripped": "documentation issue 17914",
            "focused": false
        })
    }

    #[test]
    fn parses_agent_list_entries() {
        // Wrapped in an object (as `herdr agent list` result) or a bare array.
        for result in [json!({"agents": [sample_entry()]}), json!([sample_entry()])] {
            let agents = parse_agent_list(&result);
            assert_eq!(
                agents,
                vec![AgentInfo {
                    pane_id: "w1:p7".into(),
                    kind: "claude".into(),
                    session_id: "13bb6c2a-1b44-4485-a9c3-e02f5d662dfd".into(),
                    status: AgentStatus::Working,
                }]
            );
        }
    }

    #[test]
    fn skips_panes_without_agent_session() {
        let result = json!([{"pane_id": "w1:p2", "agent_status": "unknown"}]);
        assert!(parse_agent_list(&result).is_empty());
    }

    #[test]
    fn status_strings_map_and_unknown_is_never_done() {
        assert_eq!(AgentStatus::parse("working"), AgentStatus::Working);
        assert_eq!(AgentStatus::parse("blocked"), AgentStatus::Blocked);
        assert_eq!(AgentStatus::parse("done"), AgentStatus::Done);
        assert_eq!(AgentStatus::parse("idle"), AgentStatus::Idle);
        assert_eq!(AgentStatus::parse("something-new"), AgentStatus::Unknown);
    }

    #[test]
    fn resume_args_per_kind() {
        assert_eq!(
            resume_args("claude", "abc-123"),
            Some(vec!["--resume".into(), "abc-123".into()])
        );
        assert_eq!(resume_args("mystery-agent", "abc"), None);
        let a = crate::feed::AgentRef::parse("claude:abc-123").unwrap();
        assert_eq!(resume_hint(&a), "claude --resume abc-123");
        let b = crate::feed::AgentRef::parse("mystery:zz").unwrap();
        assert_eq!(resume_hint(&b), "mystery:zz");
    }

    #[test]
    fn open_resume_tab_composes_tab_create_and_agent_start() {
        let mut fake = FakeHerdr::default();
        fake.open_resume_tab("claude", "abc-123").unwrap();
        assert_eq!(
            fake.log.borrow().as_slice(),
            [
                "create_tab resume claude",
                "agent_start feedr-resume-abc-123 claude w9:p9 --resume abc-123",
            ]
        );
        // Unknown kinds can't be resumed; failure surfaces before any call:
        assert!(fake.open_resume_tab("mystery", "x").is_err());
        assert_eq!(fake.log.borrow().len(), 2);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test tui::socket`
Expected: FAIL — nothing defined yet (compile errors).

- [ ] **Step 3: Implement the protocol layer** (top of `src/tui/socket.rs`)

```rust
use anyhow::Result;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl AgentStatus {
    /// `unknown` (and anything unrecognized) must never render as done —
    /// research-doc caveat.
    pub fn parse(s: &str) -> Self {
        match s {
            "idle" => AgentStatus::Idle,
            "working" => AgentStatus::Working,
            "blocked" => AgentStatus::Blocked,
            "done" => AgentStatus::Done,
            _ => AgentStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub pane_id: String,
    pub kind: String,
    /// `agent_session.value` — joins to the feed's `@agent(kind:id)` id part.
    pub session_id: String,
    pub status: AgentStatus,
}

/// The entries inside a method result: a bare array, or the first array-valued
/// field of a result object (the research doc pins the entry shape but not the
/// envelope, so read tolerantly).
pub(crate) fn result_entries(result: &Value) -> Vec<&Value> {
    if let Some(a) = result.as_array() {
        return a.iter().collect();
    }
    if let Some(obj) = result.as_object() {
        for v in obj.values() {
            if let Some(a) = v.as_array() {
                return a.iter().collect();
            }
        }
    }
    Vec::new()
}

pub fn parse_agent_list(result: &Value) -> Vec<AgentInfo> {
    result_entries(result)
        .into_iter()
        .filter_map(|v| {
            Some(AgentInfo {
                pane_id: v.get("pane_id")?.as_str()?.to_string(),
                kind: v.get("agent")?.as_str()?.to_string(),
                session_id: v.get("agent_session")?.get("value")?.as_str()?.to_string(),
                status: AgentStatus::parse(
                    v.get("agent_status").and_then(|s| s.as_str()).unwrap_or("unknown"),
                ),
            })
        })
        .collect()
}

/// Pass-through args that resume a session for an agent kind (spec §3:
/// "kind prefix selects the command"). These ride in `agent.start`'s `args`
/// field, appended to the agent binary the kind selects.
pub fn resume_args(kind: &str, session_id: &str) -> Option<Vec<String>> {
    match kind {
        "claude" => Some(vec!["--resume".into(), session_id.into()]),
        _ => None,
    }
}

/// Human-readable fallback shown in the status line when herdr can't do it.
pub fn resume_hint(agent: &crate::feed::AgentRef) -> String {
    resume_args(&agent.kind, &agent.id)
        .map(|a| format!("{} {}", agent.kind, a.join(" ")))
        .unwrap_or_else(|| agent.to_string())
}

/// Everything the sidebar asks of herdr, behind a trait so tests fake it.
/// Verbs verified against live herdr 0.9.0 (`herdr api schema --json`).
pub trait Herdr {
    fn list_agents(&mut self) -> Result<Vec<AgentInfo>>;
    /// `agent.focus {target}` — jump the herdr UI to the agent's pane.
    fn focus_agent(&mut self, target: &str) -> Result<()>;
    /// `pane.focus {pane_id}` — plain pane focus (used by the dock launcher).
    fn focus_pane(&mut self, pane_id: &str) -> Result<()>;
    /// `tab.create` (TabCreateParams: workspace_id/cwd/env/label/focus — it
    /// spawns a shell pane, never a command). Returns the new pane's id.
    fn create_tab(&mut self, label: &str) -> Result<String>;
    /// `agent.start` (AgentStartParams: {name, kind, pane_id, args[],
    /// timeout_ms}) — start an agent in a pane; `args` pass through to the
    /// agent binary.
    fn agent_start(&mut self, name: &str, kind: &str, pane_id: &str, args: &[String])
        -> Result<()>;

    /// Resume a session in a fresh tab (spec §3: @agent click with the pane
    /// gone): create a tab, then start the agent in its pane with the kind's
    /// resume args. `agent.start` succeeding means herdr *detected* the
    /// agent — the item's @agent link is live again and its status glyph
    /// returns on the next resync. Provided so every impl (and the fake)
    /// shares one composition.
    fn open_resume_tab(&mut self, kind: &str, session_id: &str) -> Result<()> {
        let args = resume_args(kind, session_id)
            .ok_or_else(|| anyhow::anyhow!("no resume command for agent kind \"{kind}\""))?;
        let pane_id = self.create_tab(&format!("resume {kind}"))?;
        let name = format!(
            "feedr-resume-{}",
            session_id.chars().take(8).collect::<String>()
        );
        self.agent_start(&name, kind, &pane_id, &args)
    }
}

#[cfg(test)]
#[derive(Default, Clone)]
pub struct FakeHerdr {
    pub fail: bool,
    pub agents: Vec<AgentInfo>,
    /// Shared so tests can keep a clone and inspect calls after boxing.
    pub log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
}

#[cfg(test)]
impl Herdr for FakeHerdr {
    fn list_agents(&mut self) -> Result<Vec<AgentInfo>> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        Ok(self.agents.clone())
    }
    fn focus_agent(&mut self, target: &str) -> Result<()> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        self.log.borrow_mut().push(format!("focus_agent {target}"));
        Ok(())
    }
    fn focus_pane(&mut self, pane_id: &str) -> Result<()> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        self.log.borrow_mut().push(format!("focus_pane {pane_id}"));
        Ok(())
    }
    fn create_tab(&mut self, label: &str) -> Result<String> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        self.log.borrow_mut().push(format!("create_tab {label}"));
        Ok("w9:p9".into())
    }
    fn agent_start(&mut self, name: &str, kind: &str, pane_id: &str, args: &[String]) -> Result<()> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        self.log
            .borrow_mut()
            .push(format!("agent_start {name} {kind} {pane_id} {}", args.join(" ")));
        Ok(())
    }
    // open_resume_tab: provided method — the fake exercises the real
    // composition, so tests see the create_tab + agent_start sequence.
}
```

Add `pub mod socket;` as the first line of `src/tui/mod.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test tui::socket`
Expected: PASS — 5 tests.

- [ ] **Step 5: Commit**

```bash
git add src/tui
git commit -m "feat: herdr socket protocol types and Herdr trait with fake"
```

---

### Task 4: App state — Row model, ItemKey, relocate

**Files:**
- Create: `src/tui/app.rs`
- Modify: `src/tui/mod.rs` (add `pub mod app;`)

The pure heart of the sidebar: `build_rows` flattens the `Document` into display rows (sections, items, `@agent` sub-lines, the `+ add` row) and counts archived items for `Done (n)`; `ItemKey`/`relocate` give actions a stable handle across concurrent edits (Decision 3).

- [ ] **Step 1: Write the failing tests** (bottom of the new `src/tui/app.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::parse::parse;

    pub(crate) const SAMPLE: &str = "\
# Feed

- [ ] Fix auth redirect loop
  Repro: bounces forever.
- [~] Migrate CI @agent(claude:0198f3ab)

## Later

- [ ] Evaluate pnpm catalogs

## Agent

- [?] Add retry to deploy test @agent(claude:0198f3ab)

# Done

## Feed

- [x] Old thing @done(2026-09-01)
- [x] Older thing @done(2026-08-20)
";

    #[test]
    fn rows_flatten_sections_items_agent_lines_and_add() {
        let doc = parse(SAMPLE);
        let (rows, done_count) = build_rows(&doc);
        assert_eq!(done_count, 2);
        let kinds: Vec<String> = rows
            .iter()
            .map(|r| match r {
                Row::Section(t) => format!("S:{t}"),
                Row::Item { key, .. } => format!("I:{}:{}", key.state.to_char(), key.title),
                Row::AgentLine { agent, .. } => format!("A:{}", agent.kind),
                Row::Add => "+".into(),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "I: :Fix auth redirect loop",
                "I:~:Migrate CI",
                "A:claude",
                "S:Later",
                "I: :Evaluate pnpm catalogs",
                "S:Agent",
                "I:?:Add retry to deploy test",
                "A:claude",
                "+",
            ]
        );
    }

    #[test]
    fn relocate_finds_by_title_and_state_skipping_archive() {
        let doc = parse(SAMPLE);
        let key = ItemKey { title: "Migrate CI".into(), state: State::InProgress };
        let i = relocate(&doc, &key).unwrap();
        assert!(matches!(&doc.nodes[i], Node::Item(it) if it.title == "Migrate CI"));
        // State mismatch (item advanced under us) → miss:
        let stale = ItemKey { title: "Migrate CI".into(), state: State::Open };
        assert_eq!(relocate(&doc, &stale), None);
        // Archived items are never relocated:
        let archived = ItemKey { title: "Old thing".into(), state: State::Done };
        assert_eq!(relocate(&doc, &archived), None);
    }

    #[test]
    fn section_choices_list_first_then_named_then_agent() {
        let doc = parse(SAMPLE);
        let c = section_choices(&doc);
        assert_eq!(
            c,
            vec![
                SectionChoice::FirstHuman,
                SectionChoice::Named("Later".into()),
                SectionChoice::Agent,
            ]
        );
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test tui::app`
Expected: FAIL — compile errors (nothing defined).

- [ ] **Step 3: Implement** (top of `src/tui/app.rs`)

```rust
use crate::config::SidebarConfig;
use crate::feed::ops::{self, Zone};
use crate::feed::{AgentRef, Document, Item, Node, State};
use crate::tui::socket::{AgentInfo, Herdr};
use std::collections::HashMap;
use std::path::PathBuf;

/// Stable-enough handle on an item across concurrent edits: exact title +
/// state, re-matched against a fresh read on every action. A miss means the
/// item changed on disk — the action is dropped with a status message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemKey {
    pub title: String,
    pub state: State,
}

impl ItemKey {
    pub fn of(it: &Item) -> Self {
        ItemKey { title: it.title.clone(), state: it.state }
    }
}

pub fn relocate(doc: &Document, key: &ItemKey) -> Option<usize> {
    doc.nodes.iter().enumerate().find_map(|(i, n)| match n {
        Node::Item(it)
            if ops::zone_of(doc, i) != Zone::Archive
                && it.title == key.title
                && it.state == key.state =>
        {
            Some(i)
        }
        _ => None,
    })
}

/// One display row of the sidebar list.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Section(String),
    Item { key: ItemKey, agent: Option<AgentRef> },
    AgentLine { key: ItemKey, agent: AgentRef },
    Add,
}

/// Flatten the active feed into display rows; count archived items for Done(n).
pub fn build_rows(doc: &Document) -> (Vec<Row>, usize) {
    let mut rows = Vec::new();
    let mut done_count = 0usize;
    for (i, n) in doc.nodes.iter().enumerate() {
        let archived = ops::zone_of(doc, i) == Zone::Archive;
        match n {
            Node::Item(_) if archived => done_count += 1,
            Node::Heading { level: 2, text } if !archived => {
                rows.push(Row::Section(text.clone()));
            }
            Node::Item(it) => {
                let key = ItemKey::of(it);
                rows.push(Row::Item { key: key.clone(), agent: it.agent.clone() });
                if let Some(a) = &it.agent {
                    rows.push(Row::AgentLine { key, agent: a.clone() });
                }
            }
            _ => {}
        }
    }
    rows.push(Row::Add);
    (rows, done_count)
}

/// Where a created item can go: the first human region, a named `##` human
/// section, or the reserved `## Agent` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionChoice {
    FirstHuman,
    Named(String),
    Agent,
}

pub fn section_choices(doc: &Document) -> Vec<SectionChoice> {
    let mut v = vec![SectionChoice::FirstHuman];
    for (i, n) in doc.nodes.iter().enumerate() {
        if let Node::Heading { level: 2, text } = n {
            if ops::zone_of(doc, i) != Zone::Archive && !text.eq_ignore_ascii_case("Agent") {
                v.push(SectionChoice::Named(text.clone()));
            }
        }
    }
    v.push(SectionChoice::Agent);
    v
}

pub struct App {
    pub feed_path: PathBuf,
    pub cfg: SidebarConfig,
    pub doc: Document,
    pub rows: Vec<Row>,
    pub done_count: usize,
    pub scroll: usize,
    pub collapsed: bool,
    /// agent session id → live info from the herdr socket (empty when absent).
    pub statuses: HashMap<String, AgentInfo>,
    pub status_msg: Option<String>,
    pub should_quit: bool,
    pub herdr: Box<dyn Herdr>,
}

impl App {
    pub fn new(feed_path: PathBuf, cfg: SidebarConfig, herdr: Box<dyn Herdr>) -> Self {
        App {
            feed_path,
            cfg,
            doc: Document::default(),
            rows: Vec::new(),
            done_count: 0,
            scroll: 0,
            collapsed: false,
            statuses: HashMap::new(),
            status_msg: None,
            should_quit: false,
            herdr,
        }
    }

    pub fn rebuild(&mut self) {
        let (rows, done) = build_rows(&self.doc);
        self.rows = rows;
        self.done_count = done;
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(1));
    }

    /// Re-read the feed from disk (missing file = empty feed; any other read
    /// error keeps the old doc and reports — same policy as the CLI, so a bad
    /// read can never lead to clobbering the real file later).
    pub fn reload(&mut self) {
        match std::fs::read_to_string(&self.feed_path) {
            Ok(t) => self.doc = crate::feed::parse::parse(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.doc = Document::default();
            }
            Err(e) => {
                self.status_msg = Some(format!("cannot read feed: {e}"));
                return;
            }
        }
        self.rebuild();
    }
}
```

Add `pub mod app;` to `src/tui/mod.rs` under `pub mod socket;`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test tui::app`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src/tui
git commit -m "feat: sidebar app state — row model, item keys, relocate"
```

---

### Task 5: List rendering (TestBackend)

**Files:**
- Create: `src/tui/view.rs`, `src/tui/test_util.rs`
- Modify: `src/tui/mod.rs` (add `pub mod view;` and `#[cfg(test)] pub mod test_util;`)

Layout (spec §3): row 0 toolbar `« sweep file`; scrollable list; second-to-last row bottom-pinned `Done (n)`; last row transient status message. Collapsed: the whole pane renders as a rail showing `»`. Glyphs per Decision 4. Titles ellipsized to the pane width (spec: "~20 cols").

- [ ] **Step 1: Create `src/tui/test_util.rs`** (test-only helper, no TDD on itself)

```rust
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Draw the app into an in-memory buffer and return each row as a
/// right-trimmed string.
pub fn render_to_strings(app: &crate::tui::app::App, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| crate::tui::view::draw(f, app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}
```

Add to `src/tui/mod.rs`:

```rust
#[cfg(test)]
pub mod test_util;
```

- [ ] **Step 2: Write the failing tests** (bottom of the new `src/tui/view.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Side, SidebarConfig};
    use crate::feed::parse::parse;
    use crate::tui::app::App;
    use crate::tui::socket::{AgentInfo, AgentStatus, FakeHerdr};
    use crate::tui::test_util::render_to_strings;

    fn test_cfg() -> SidebarConfig {
        SidebarConfig { side: Side::Left, width: 0.18, auto_dock: false }
    }

    fn app_with(text: &str) -> App {
        let mut app = App::new("/nonexistent/feed.md".into(), test_cfg(), Box::new(FakeHerdr::default()));
        app.doc = parse(text);
        app.rebuild();
        app
    }

    const SAMPLE: &str = "\
# Feed

- [ ] Fix auth redirect loop
- [~] Migrate CI @agent(claude:0198f3ab)

## Agent

- [?] Add retry to deploy test @agent(claude:0198f3ab)

# Done

## Feed

- [x] Old thing @done(2026-09-01)
";

    #[test]
    fn renders_toolbar_list_done_and_status_rows() {
        let mut app = app_with(SAMPLE);
        app.statuses.insert(
            "0198f3ab".into(),
            AgentInfo {
                pane_id: "w1:p7".into(),
                kind: "claude".into(),
                session_id: "0198f3ab".into(),
                status: AgentStatus::Working,
            },
        );
        app.status_msg = Some("hello".into());
        let rows = render_to_strings(&app, 20, 12);
        assert_eq!(rows[0], "« sweep file");
        assert_eq!(rows[1], "· Fix auth redirect…"); // ellipsized at 20 cols
        assert_eq!(rows[2], "~ Migrate CI");
        assert_eq!(rows[3], "  @claude >"); // live working glyph
        assert_eq!(rows[4], "Agent");
        assert_eq!(rows[5], "? Add retry to depl…");
        assert_eq!(rows[6], "  @claude >");
        assert_eq!(rows[7], "+ add");
        assert_eq!(rows[10], "Done (1)"); // bottom-pinned, h-2
        assert_eq!(rows[11], "hello"); // status line, h-1
    }

    #[test]
    fn agent_glyph_hidden_when_socket_absent() {
        let app = app_with(SAMPLE); // statuses empty
        let rows = render_to_strings(&app, 20, 12);
        assert_eq!(rows[3], "  @claude"); // sub-line still shown, no glyph
    }

    #[test]
    fn collapsed_renders_rail() {
        let mut app = app_with(SAMPLE);
        app.collapsed = true;
        let rows = render_to_strings(&app, 3, 6);
        assert_eq!(rows[0], "»");
        assert!(rows[1..].iter().all(|r| r.is_empty()));
    }

    #[test]
    fn list_scrolls() {
        let mut app = app_with(SAMPLE);
        app.scroll = 2;
        let rows = render_to_strings(&app, 20, 12);
        assert_eq!(rows[1], "  @claude"); // row index 2 of the row model
    }

    #[test]
    fn ellipsize_is_char_safe() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("exactly-ten", 11), "exactly-ten");
        assert_eq!(ellipsize("exactly-eleven!", 11), "exactly-el…");
        assert_eq!(ellipsize("x", 0), "");
    }
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test tui::view`
Expected: FAIL — compile errors (`draw`, `ellipsize` not defined).

- [ ] **Step 4: Implement** (top of `src/tui/view.rs`)

```rust
use crate::feed::State;
use crate::tui::app::{App, Row};
use crate::tui::socket::AgentStatus;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// Toolbar text; the click spans in input.rs must match these columns:
/// « at 0, "sweep" at 2..=6, "file" at 8..=11.
pub const TOOLBAR: &str = "« sweep file";

pub fn state_glyph(s: State) -> char {
    match s {
        State::Open => '·',
        State::InProgress => '~',
        State::Review => '?',
        State::Done => 'x',
    }
}

/// Display-only live status (spec §3: never written to the file).
/// Unknown renders nothing — it must not look like done.
pub fn status_glyph(s: AgentStatus) -> Option<char> {
    match s {
        AgentStatus::Working => Some('>'),
        AgentStatus::Blocked => Some('!'),
        AgentStatus::Done => Some('✓'),
        AgentStatus::Idle => Some('-'),
        AgentStatus::Unknown => None,
    }
}

pub fn ellipsize(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else if width == 0 {
        String::new()
    } else {
        let mut t: String = s.chars().take(width - 1).collect();
        t.push('…');
        t
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if app.collapsed {
        // ~3-col rail: click anywhere restores (input.rs).
        f.render_widget(Paragraph::new("»"), area);
        return;
    }
    let [toolbar, list, done, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(TOOLBAR), toolbar);

    let w = area.width as usize;
    let lines: Vec<Line> = app
        .rows
        .iter()
        .skip(app.scroll)
        .take(list.height as usize)
        .map(|r| row_line(app, r, w))
        .collect();
    f.render_widget(Paragraph::new(lines), list);

    f.render_widget(
        Paragraph::new(format!("Done ({})", app.done_count))
            .style(Style::default().add_modifier(Modifier::BOLD)),
        done,
    );
    f.render_widget(
        Paragraph::new(app.status_msg.clone().unwrap_or_default()),
        status,
    );
}

fn row_line(app: &App, row: &Row, w: usize) -> Line<'static> {
    match row {
        Row::Section(t) => Line::styled(
            ellipsize(t, w),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Row::Item { key, .. } => Line::raw(format!(
            "{} {}",
            state_glyph(key.state),
            ellipsize(&key.title, w.saturating_sub(2)),
        )),
        Row::AgentLine { agent, .. } => {
            let glyph = app
                .statuses
                .get(&agent.id)
                .and_then(|i| status_glyph(i.status))
                .map(|g| format!(" {g}"))
                .unwrap_or_default();
            Line::raw(format!(
                "  @{}{}",
                ellipsize(&agent.kind, w.saturating_sub(4 + glyph.chars().count())),
                glyph,
            ))
        }
        Row::Add => Line::raw("+ add"),
    }
}
```

Add `pub mod view;` to `src/tui/mod.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test tui::view`
Expected: PASS — 5 tests. (If the two ellipsis assertions are off by one character, recount against `ellipsize`: it keeps `width - 1` chars + `…` — fix the *test literal*, not the function, and only if the function matches this spec.)

- [ ] **Step 6: Commit**

```bash
git add src/tui
git commit -m "feat: sidebar list rendering with glyphs, rail, done row"
```

---

### Task 6: Input translation (pure hit-testing)

**Files:**
- Create: `src/tui/input.rs`
- Modify: `src/tui/app.rs` (add the `Action` enum), `src/tui/mod.rs` (add `pub mod input;`)

Every spec §3 interaction becomes an `Action`. `translate` is pure: `(event, &App, terminal size) → Option<Action>`. Geometry mirrors Task 5's layout: y=0 toolbar, list rows 1..=h-3, Done row at h-2.

- [ ] **Step 1: Add `Action` to `src/tui/app.rs`** (below `SectionChoice`; no test yet — it's exercised by translate's tests)

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Quit,
    ToggleCollapse,
    Sweep,
    OpenFileView,
    OpenEditor,
    Advance(ItemKey),
    OpenEdit(ItemKey),
    OpenCreate,
    OpenDoneView,
    AgentClick(ItemKey),
    ScrollUp,
    ScrollDown,
}
```

- [ ] **Step 2: Write the failing tests** (bottom of the new `src/tui/input.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Side, SidebarConfig};
    use crate::feed::parse::parse;
    use crate::tui::app::{Action, App, ItemKey};
    use crate::feed::State;
    use crate::tui::socket::FakeHerdr;
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    fn app_with(text: &str) -> App {
        let cfg = SidebarConfig { side: Side::Left, width: 0.18, auto_dock: false };
        let mut app = App::new("/nonexistent/feed.md".into(), cfg, Box::new(FakeHerdr::default()));
        app.doc = parse(text);
        app.rebuild();
        app
    }

    const SAMPLE: &str = "\
# Feed

- [ ] Alpha
- [?] Beta @agent(claude:abc)

# Done

## Feed

- [x] Old @done(2026-09-01)
";
    // Rows: 0 I:Alpha · 1 I:Beta · 2 A:claude · 3 Add

    fn click(x: u16, y: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    const SIZE: (u16, u16) = (20, 10); // list rows y=1..=7, Done row y=8, status y=9

    #[test]
    fn keys_quit_add_editor_scroll() {
        let app = app_with(SAMPLE);
        assert_eq!(translate(&key('q'), &app, SIZE), Some(Action::Quit));
        assert_eq!(translate(&key('a'), &app, SIZE), Some(Action::OpenCreate));
        assert_eq!(translate(&key('e'), &app, SIZE), Some(Action::OpenEditor));
        assert_eq!(
            translate(&Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)), &app, SIZE),
            Some(Action::ScrollDown)
        );
    }

    #[test]
    fn toolbar_clicks() {
        let app = app_with(SAMPLE);
        assert_eq!(translate(&click(0, 0), &app, SIZE), Some(Action::ToggleCollapse));
        assert_eq!(translate(&click(4, 0), &app, SIZE), Some(Action::Sweep));
        assert_eq!(translate(&click(9, 0), &app, SIZE), Some(Action::OpenFileView));
        assert_eq!(translate(&click(15, 0), &app, SIZE), None); // dead zone
    }

    #[test]
    fn checkbox_click_vs_title_click() {
        let app = app_with(SAMPLE);
        let alpha = ItemKey { title: "Alpha".into(), state: State::Open };
        assert_eq!(translate(&click(0, 1), &app, SIZE), Some(Action::Advance(alpha.clone())));
        assert_eq!(translate(&click(1, 1), &app, SIZE), Some(Action::Advance(alpha.clone())));
        assert_eq!(translate(&click(5, 1), &app, SIZE), Some(Action::OpenEdit(alpha)));
    }

    #[test]
    fn agent_line_add_row_and_done_row_clicks() {
        let app = app_with(SAMPLE);
        let beta = ItemKey { title: "Beta".into(), state: State::Review };
        assert_eq!(translate(&click(4, 3), &app, SIZE), Some(Action::AgentClick(beta)));
        assert_eq!(translate(&click(2, 4), &app, SIZE), Some(Action::OpenCreate)); // + add
        assert_eq!(translate(&click(3, 8), &app, SIZE), Some(Action::OpenDoneView));
        assert_eq!(translate(&click(3, 6), &app, SIZE), None); // empty list space
    }

    #[test]
    fn scroll_offsets_hit_testing_and_collapsed_restores() {
        let mut app = app_with(SAMPLE);
        app.scroll = 2;
        // y=1 now hits row 2 (the agent line):
        let beta = ItemKey { title: "Beta".into(), state: State::Review };
        assert_eq!(translate(&click(4, 1), &app, SIZE), Some(Action::AgentClick(beta)));
        app.collapsed = true;
        assert_eq!(translate(&click(1, 4), &app, SIZE), Some(Action::ToggleCollapse));
    }
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test tui::input`
Expected: FAIL — `translate` not defined.

- [ ] **Step 4: Implement** (top of `src/tui/input.rs`)

```rust
use crate::tui::app::{Action, App, Row};
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};

/// Pure event → action mapping. `size` is the terminal (width, height);
/// geometry must mirror view::draw: y=0 toolbar, list at y=1..=h-3,
/// Done(n) at h-2, status line at h-1.
pub fn translate(ev: &Event, app: &App, size: (u16, u16)) -> Option<Action> {
    let (_w, h) = size;
    match ev {
        Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('a') => Some(Action::OpenCreate),
            KeyCode::Char('e') => Some(Action::OpenEditor),
            KeyCode::Up => Some(Action::ScrollUp),
            KeyCode::Down => Some(Action::ScrollDown),
            _ => None,
        },
        Event::Mouse(m) => match m.kind {
            MouseEventKind::ScrollUp => Some(Action::ScrollUp),
            MouseEventKind::ScrollDown => Some(Action::ScrollDown),
            MouseEventKind::Down(MouseButton::Left) => click(app, m.column, m.row, h),
            _ => None,
        },
        _ => None,
    }
}

fn click(app: &App, x: u16, y: u16, h: u16) -> Option<Action> {
    if app.collapsed {
        // Any click on the rail restores the sidebar.
        return Some(Action::ToggleCollapse);
    }
    if y == 0 {
        // Toolbar columns must match view::TOOLBAR ("« sweep file").
        return match x {
            0 => Some(Action::ToggleCollapse),
            2..=6 => Some(Action::Sweep),
            8..=11 => Some(Action::OpenFileView),
            _ => None,
        };
    }
    if h >= 2 && y == h - 2 {
        return Some(Action::OpenDoneView);
    }
    if h >= 4 && (1..=h - 3).contains(&y) {
        let idx = app.scroll + (y as usize - 1);
        return match app.rows.get(idx)? {
            Row::Section(_) => None,
            Row::Item { key, .. } => Some(if x <= 1 {
                // Checkbox click: state-advance (spec §3).
                Action::Advance(key.clone())
            } else {
                Action::OpenEdit(key.clone())
            }),
            Row::AgentLine { key, .. } => Some(Action::AgentClick(key.clone())),
            Row::Add => Some(Action::OpenCreate),
        };
    }
    None
}
```

Add `pub mod input;` to `src/tui/mod.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test tui::input`
Expected: PASS — 5 tests.

- [ ] **Step 6: Commit**

```bash
git add src/tui
git commit -m "feat: sidebar input translation with mouse hit-testing"
```

---

### Task 7: Actions and feed mutations + real event loop

**Files:**
- Modify: `src/tui/app.rs` (add `apply` and helpers), `src/tui/mod.rs` (real event loop)

The concurrency-critical path (spec §6): every mutation reads the feed **fresh**, relocates the item, applies exactly one `ops::` change with `Authority::Human`, and writes with `save_atomic`. A relocate miss drops the action with a status message.

- [ ] **Step 1: Write the failing tests** (append inside `src/tui/app.rs`'s `tests` module)

```rust
    use crate::config::{Side, SidebarConfig};
    use crate::tui::socket::{AgentInfo, AgentStatus, FakeHerdr};

    fn test_cfg() -> SidebarConfig {
        SidebarConfig { side: Side::Left, width: 0.18, auto_dock: false }
    }

    /// App over a real temp feed file, plus a handle on the fake herdr's log.
    fn app_on_disk(text: &str) -> (App, FakeHerdr, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.md");
        std::fs::write(&path, text).unwrap();
        let fake = FakeHerdr::default();
        let mut app = App::new(path, test_cfg(), Box::new(fake.clone()));
        app.reload();
        (app, fake, dir)
    }

    fn feed_text(app: &App) -> String {
        std::fs::read_to_string(&app.feed_path).unwrap()
    }

    #[test]
    fn advance_cycles_and_review_click_accepts() {
        let (mut app, _fake, _dir) =
            app_on_disk("# Feed\n\n- [ ] Alpha\n- [~] Beta\n- [?] Gamma\n- [x] Delta\n");
        for (title, from, expect) in [
            ("Alpha", State::Open, "- [~] Alpha"),        // [ ] → [~]
            ("Beta", State::InProgress, "- [x] Beta"),    // [~] → [x]
            ("Gamma", State::Review, "- [x] Gamma"),      // [?] click = accept → [x]
            ("Delta", State::Done, "- [ ] Delta"),        // [x] → [ ]
        ] {
            app.apply(Action::Advance(ItemKey { title: title.into(), state: from }));
            assert!(feed_text(&app).contains(expect), "want {expect} in:\n{}", feed_text(&app));
        }
    }

    #[test]
    fn advance_never_produces_review_state() {
        // No transition targets [?] — that's the agent's signal (spec §3).
        let (mut app, _fake, _dir) = app_on_disk("- [ ] A\n");
        app.apply(Action::Advance(ItemKey { title: "A".into(), state: State::Open }));
        app.apply(Action::Advance(ItemKey { title: "A".into(), state: State::InProgress }));
        app.apply(Action::Advance(ItemKey { title: "A".into(), state: State::Done }));
        assert_eq!(feed_text(&app), "- [ ] A\n"); // full cycle, never [?]
    }

    #[test]
    fn stale_key_drops_action_with_message() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] Alpha\n");
        // Simulate a concurrent agent claim between render and click:
        std::fs::write(&app.feed_path, "- [~] Alpha @agent(claude:abc)\n").unwrap();
        app.apply(Action::Advance(ItemKey { title: "Alpha".into(), state: State::Open }));
        assert!(feed_text(&app).contains("- [~] Alpha @agent(claude:abc)")); // untouched
        assert!(app.status_msg.as_deref().unwrap().contains("changed on disk"));
    }

    #[test]
    fn sweep_action_archives_and_reports() {
        let (mut app, _fake, _dir) = app_on_disk("# Feed\n\n- [x] Ship it\n- [ ] Keep\n");
        app.apply(Action::Sweep);
        let text = feed_text(&app);
        assert!(text.contains("# Done"));
        assert!(text.contains("- [x] Ship it @done("));
        assert!(app.status_msg.as_deref().unwrap().contains("swept 1"));
        assert_eq!(app.done_count, 1); // reloaded after the write
    }

    #[test]
    fn agent_click_focuses_live_pane() {
        let (mut app, fake, _dir) = app_on_disk("- [~] Beta @agent(claude:abc)\n");
        app.statuses.insert(
            "abc".into(),
            AgentInfo {
                pane_id: "w1:p7".into(),
                kind: "claude".into(),
                session_id: "abc".into(),
                status: AgentStatus::Working,
            },
        );
        app.apply(Action::AgentClick(ItemKey { title: "Beta".into(), state: State::InProgress }));
        assert_eq!(fake.log.borrow().as_slice(), ["focus_agent w1:p7"]);
    }

    #[test]
    fn agent_click_without_pane_resumes_session() {
        let (mut app, fake, _dir) = app_on_disk("- [~] Beta @agent(claude:abc)\n");
        app.apply(Action::AgentClick(ItemKey { title: "Beta".into(), state: State::InProgress }));
        // Two-step resume flow (tab.create then agent.start), via the
        // trait's provided open_resume_tab:
        assert_eq!(
            fake.log.borrow().as_slice(),
            [
                "create_tab resume claude",
                "agent_start feedr-resume-abc claude w9:p9 --resume abc",
            ]
        );
    }

    #[test]
    fn agent_click_degrades_to_status_message_when_herdr_absent() {
        let (mut app, _fake, _dir) = app_on_disk("- [~] Beta @agent(claude:abc)\n");
        app.herdr = Box::new(FakeHerdr { fail: true, ..FakeHerdr::default() });
        app.apply(Action::AgentClick(ItemKey { title: "Beta".into(), state: State::InProgress }));
        let msg = app.status_msg.as_deref().unwrap();
        assert!(msg.contains("claude --resume abc"), "got: {msg}");
    }

    #[test]
    fn quit_scroll_collapse_flags() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] A\n- [ ] B\n");
        app.apply(Action::ScrollDown);
        assert_eq!(app.scroll, 1);
        app.apply(Action::ScrollUp);
        app.apply(Action::ScrollUp); // clamped at 0
        assert_eq!(app.scroll, 0);
        app.apply(Action::ToggleCollapse);
        assert!(app.collapsed);
        app.apply(Action::Quit);
        assert!(app.should_quit);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test tui::app`
Expected: FAIL — `App::apply` not defined.

- [ ] **Step 3: Implement in `src/tui/app.rs`.** Add imports at the top: `use crate::feed::ops::Authority;` and `use crate::feed::write::save_atomic;` and `use anyhow::Result;`. Add to `impl App`:

```rust
    /// Read fresh → apply one change → atomic save → reload. The closure
    /// returns an optional status message. This is the ONLY path that writes
    /// the feed from the TUI (spec §6).
    fn with_feed<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Document) -> Result<Option<String>>,
    {
        let text = match std::fs::read_to_string(&self.feed_path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                self.status_msg = Some(format!("cannot read feed: {e}"));
                return;
            }
        };
        let mut doc = crate::feed::parse::parse(&text);
        match f(&mut doc) {
            Ok(msg) => match save_atomic(&doc, &self.feed_path) {
                Ok(()) => self.status_msg = msg,
                Err(e) => self.status_msg = Some(format!("save failed: {e}")),
            },
            Err(e) => self.status_msg = Some(e.to_string()),
        }
        self.reload();
    }

    pub fn apply(&mut self, action: Action) {
        self.status_msg = None;
        match action {
            Action::Quit => self.should_quit = true,
            Action::ToggleCollapse => self.collapsed = !self.collapsed,
            Action::ScrollUp => self.scroll = self.scroll.saturating_sub(1),
            Action::ScrollDown => {
                if self.scroll + 1 < self.rows.len() {
                    self.scroll += 1;
                }
            }
            Action::Advance(key) => self.advance(key),
            Action::Sweep => {
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                self.with_feed(|doc| {
                    let n = doc
                        .nodes
                        .iter()
                        .enumerate()
                        .filter(|(i, node)| {
                            matches!(node, Node::Item(it) if it.state == State::Done)
                                && ops::zone_of(doc, *i) != Zone::Archive
                        })
                        .count();
                    ops::sweep(doc, &today);
                    Ok(Some(format!("swept {n} item(s)")))
                });
            }
            Action::AgentClick(key) => self.agent_click(key),
            // Wired in later tasks (modal: Task 9; viewers/editor: Task 10):
            Action::OpenEdit(_)
            | Action::OpenCreate
            | Action::OpenDoneView
            | Action::OpenFileView
            | Action::OpenEditor => {}
        }
    }

    /// Checkbox click state-advance (spec §3): [ ]→[~]→[x]→[ ]; on [?] the
    /// click means "accept" → [x]. No click ever produces [?].
    fn advance(&mut self, key: ItemKey) {
        let next = match key.state {
            State::Open => State::InProgress,
            State::InProgress => State::Done,
            State::Review => State::Done, // accept
            State::Done => State::Open,
        };
        self.with_feed(|doc| match relocate(doc, &key) {
            Some(i) => {
                ops::set_state(doc, i, next, Authority::Human)?;
                Ok(None)
            }
            None => Ok(Some("item changed on disk — click dropped".into())),
        });
    }

    /// @agent sub-line click (spec §3): pane live → agent.focus; pane gone →
    /// resume tab; herdr absent → status-line hint, feed untouched.
    fn agent_click(&mut self, key: ItemKey) {
        let Some(i) = relocate(&self.doc, &key) else { return };
        let Node::Item(it) = &self.doc.nodes[i] else { return };
        let Some(agent) = it.agent.clone() else { return };
        let pane = self.statuses.get(&agent.id).map(|a| a.pane_id.clone());
        let result = match pane {
            Some(p) => self.herdr.focus_agent(&p),
            None => self.herdr.open_resume_tab(&agent.kind, &agent.id),
        };
        if let Err(e) = result {
            self.status_msg = Some(format!(
                "{e} — resume manually: {}",
                crate::tui::socket::resume_hint(&agent)
            ));
        }
    }
```

- [ ] **Step 4: Replace `src/tui/mod.rs` with the real event loop** (full file; `AppEvent` senders arrive in Tasks 11–12, so `_tx` is unused for now):

```rust
pub mod app;
pub mod input;
pub mod socket;
pub mod view;
#[cfg(test)]
pub mod test_util;

use crate::config::SidebarConfig;
use anyhow::Result;
use app::App;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

/// Events pushed by background threads (file watcher: Task 11; socket
/// subscriber: Task 12).
pub enum AppEvent {
    FeedChanged,
    Agents(Vec<socket::AgentInfo>),
    SocketDown,
}

pub fn run(feed_path: PathBuf, cfg: SidebarConfig) -> Result<()> {
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
        if event::poll(Duration::from_millis(100))? {
            let ev = event::read()?;
            let size = terminal.size()?;
            if let Some(action) = input::translate(&ev, app, (size.width, size.height)) {
                app.apply(action);
            }
        }
    }
}
```

And add `on_event` to `impl App` in `src/tui/app.rs` (import `crate::tui::AppEvent`):

```rust
    pub fn on_event(&mut self, ev: crate::tui::AppEvent) {
        match ev {
            crate::tui::AppEvent::FeedChanged => self.reload(),
            crate::tui::AppEvent::Agents(list) => {
                self.statuses = list.into_iter().map(|a| (a.session_id.clone(), a)).collect();
            }
            crate::tui::AppEvent::SocketDown => self.statuses.clear(),
        }
    }
```

- [ ] **Step 5: Run the full suite**

Run: `cargo test`
Expected: PASS — all tests including the 8 new app tests. Then smoke-test by hand in a terminal: `FEEDR_FEED=/tmp/smoke-feed.md cargo run -- sidebar` — clicking a checkbox advances it, `q` quits, terminal restored.

- [ ] **Step 6: Commit**

```bash
git add src/tui
git commit -m "feat: sidebar actions — advance/sweep/agent-click via fresh-read atomic writes"
```

---

### Task 8: ops extensions — edit, remove, add_in_section

**Files:**
- Modify: `src/feed/ops.rs`

Three small ops the modal needs that Plan 1 didn't: in-place title/body edit, item deletion, and add-to-named-section (the section picker path — `feedr add --section` deliberately bailed in v0.1). Same invariants as existing ops: bodies go through `trim_blank_edges`, archive sections are never touched. Also a DRY refactor: `agent_section_end` becomes a call to the generalized `named_section_end`.

- [ ] **Step 1: Write the failing tests** (append inside `src/feed/ops.rs`'s `tests` module)

```rust
    #[test]
    fn edit_replaces_title_and_body_keeping_tokens() {
        let mut doc = parse("- [~] Old title @agent(claude:abc)\n  old body\n");
        let i = find(&doc, "old title").unwrap();
        edit(&mut doc, i, "New title", &["".into(), "new body".into(), "".into()]);
        assert_eq!(
            render(&doc),
            "- [~] New title @agent(claude:abc)\n  new body\n"
        );
    }

    #[test]
    fn remove_deletes_item_with_body_only() {
        let mut doc = parse("- [ ] A\n  body\n- [ ] B\n");
        let i = find(&doc, "A").unwrap();
        let removed = remove(&mut doc, i).unwrap();
        assert_eq!(removed.title, "A");
        assert_eq!(render(&doc), "- [ ] B\n");
        // Non-item index is a no-op:
        let mut doc = parse("# Feed\n- [ ] A\n");
        assert!(remove(&mut doc, 0).is_none());
        assert_eq!(render(&doc), "# Feed\n- [ ] A\n");
    }

    #[test]
    fn add_in_section_appends_to_named_human_section() {
        let mut doc = parse(SAMPLE); // has ## Agent and # Done/## Feed
        // SAMPLE has no named human section — add one:
        let mut doc2 = parse("# Feed\n\n- [ ] A\n\n## Later\n\n- [ ] L1\n\n## Agent\n\n- [ ] G\n");
        add_in_section(&mut doc2, "L2", &["ctx".into()], "Later").unwrap();
        let out = render(&doc2);
        assert!(out.contains("- [ ] L1\n- [ ] L2\n  ctx\n"), "got:\n{out}");
        // Missing section errors; archive sections never match:
        assert!(matches!(
            add_in_section(&mut doc, "X", &[], "Nope"),
            Err(OpError::NotFound(_))
        ));
        assert!(matches!(
            add_in_section(&mut doc, "X", &[], "Feed"), // only exists under # Done
            Err(OpError::NotFound(_))
        ));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test ops`
Expected: FAIL — `edit`, `remove`, `add_in_section` not defined.

- [ ] **Step 3: Implement in `src/feed/ops.rs`** (add below `add`):

```rust
/// Replace an item's title and body in place; state, agent tag, and done
/// stamp are untouched. Body goes through the same blank-edge trimming as add.
pub fn edit(doc: &mut Document, index: usize, title: &str, body: &[String]) {
    debug_assert!(matches!(doc.nodes[index], Node::Item(_)));
    if let Node::Item(it) = &mut doc.nodes[index] {
        it.title = title.to_string();
        it.body = trim_blank_edges(body);
    }
}

/// Delete an item (its body lives inside the Item node, so one removal takes
/// both). Returns the removed item, or None if the index isn't an item.
pub fn remove(doc: &mut Document, index: usize) -> Option<Item> {
    match doc.nodes.get(index) {
        Some(Node::Item(_)) => match doc.nodes.remove(index) {
            Node::Item(it) => Some(it),
            _ => unreachable!(),
        },
        _ => None,
    }
}

/// Add an open item at the end of the named active `##` section (the sidebar
/// modal's section picker). Errors when no such active section exists —
/// archive (`# Done`) subsections never match.
pub fn add_in_section(
    doc: &mut Document,
    title: &str,
    body: &[String],
    section: &str,
) -> Result<(), OpError> {
    let end = named_section_end(doc, section)
        .ok_or_else(|| OpError::NotFound(format!("section {section}")))?;
    doc.nodes.insert(
        end,
        Node::Item(Item {
            state: State::Open,
            title: title.to_string(),
            agent: None,
            done_date: None,
            body: trim_blank_edges(body),
        }),
    );
    Ok(())
}

/// Index just past the last item of the active `## <name>` section.
fn named_section_end(doc: &Document, name: &str) -> Option<usize> {
    let start = doc.nodes.iter().enumerate().find_map(|(i, n)| match n {
        Node::Heading { level: 2, text }
            if text.eq_ignore_ascii_case(name) && zone_of(doc, i) != Zone::Archive =>
        {
            Some(i)
        }
        _ => None,
    })?;
    let mut end = start + 1;
    for (i, n) in doc.nodes.iter().enumerate().skip(start + 1) {
        match n {
            Node::Heading { .. } => break,
            Node::Item(_) => end = i + 1,
            _ => {}
        }
    }
    Some(end)
}
```

Then DRY: replace the whole body of the existing `agent_section_end` with:

```rust
fn agent_section_end(doc: &Document) -> Option<usize> {
    named_section_end(doc, "Agent")
}
```

(Identical semantics: same heading match, same archive guard, same end scan — the existing `add_agent_appends_to_agent_section_creating_it` test proves it.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: PASS — 3 new ops tests, all existing ops/parse/write/cli tests still green.

- [ ] **Step 5: Commit**

```bash
git add src/feed/ops.rs
git commit -m "feat: ops edit/remove/add_in_section for the sidebar modal"
```

---

### Task 9: Edit/create modal with delete-confirm

**Files:**
- Modify: `src/tui/app.rs` (modal state machine), `src/tui/view.rs` (modal drawing), `src/tui/mod.rs` (route events to the modal)

One modal serves both edit (title click) and create (`+` row / `a`), per spec §3. Create adds a section picker; edit adds delete (with confirm). Keys: `Tab` cycles fields, `Enter` in the title jumps to the body, `Ctrl-S` saves, `Ctrl-D` asks to delete, `Esc` cancels. Text editing is `tui-textarea` (Decision 2). The modal is keyboard-driven; list mouse handling stays untouched underneath.

- [ ] **Step 1: Add modal state to `src/tui/app.rs`.** New imports at the top:

```rust
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::widgets::Block;
use tui_textarea::{CursorMove, TextArea};
```

Below `SectionChoice`, add:

```rust
pub enum Modal {
    None,
    Edit(EditModal),
    ConfirmDelete(EditModal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditFocus {
    Section,
    Title,
    Body,
}

pub struct EditModal {
    /// `Some(key)` = editing an existing item; `None` = creating.
    pub original: Option<ItemKey>,
    pub choices: Vec<SectionChoice>,
    pub choice_idx: usize,
    pub title: TextArea<'static>,
    pub body: TextArea<'static>,
    pub focus: EditFocus,
}

impl EditModal {
    pub fn create(doc: &Document) -> Self {
        let mut m = EditModal {
            original: None,
            choices: section_choices(doc),
            choice_idx: 0,
            title: TextArea::default(),
            body: TextArea::default(),
            focus: EditFocus::Section,
        };
        m.sync_blocks();
        m
    }

    pub fn edit(key: ItemKey, item: &Item) -> Self {
        let mut m = EditModal {
            original: Some(key),
            choices: Vec::new(), // moving sections is $EDITOR territory (spec §3)
            choice_idx: 0,
            title: TextArea::new(vec![item.title.clone()]),
            body: TextArea::new(item.body.clone()),
            focus: EditFocus::Title,
        };
        // TextArea::new leaves the cursor at (0,0); appending is the common
        // edit, so park it at the end deterministically.
        m.title.move_cursor(CursorMove::End);
        m.body.move_cursor(CursorMove::Bottom);
        m.body.move_cursor(CursorMove::End);
        m.sync_blocks();
        m
    }

    pub fn choice_label(&self) -> String {
        match &self.choices[self.choice_idx] {
            SectionChoice::FirstHuman => "(first section)".into(),
            SectionChoice::Named(s) => s.clone(),
            SectionChoice::Agent => "Agent".into(),
        }
    }

    pub fn cycle_focus(&mut self) {
        self.focus = match (self.focus, self.original.is_some()) {
            (EditFocus::Section, _) => EditFocus::Title,
            (EditFocus::Title, _) => EditFocus::Body,
            (EditFocus::Body, true) => EditFocus::Title, // no section field on edit
            (EditFocus::Body, false) => EditFocus::Section,
        };
        self.sync_blocks();
    }

    /// Mark the focused field's border title with `*` so focus is visible.
    pub fn sync_blocks(&mut self) {
        let t = if self.focus == EditFocus::Title { "Title*" } else { "Title" };
        let b = if self.focus == EditFocus::Body { "Body*" } else { "Body" };
        self.title.set_block(Block::bordered().title(t));
        self.body.set_block(Block::bordered().title(b));
    }

    pub fn title_text(&self) -> String {
        self.title.lines().join(" ").trim().to_string()
    }

    pub fn body_lines(&self) -> Vec<String> {
        let lines: Vec<String> = self.body.lines().to_vec();
        if lines == vec![String::new()] {
            Vec::new() // an untouched textarea is one empty line, not a body
        } else {
            lines
        }
    }
}
```

Add the field `pub modal: Modal,` to the `App` struct and `modal: Modal::None,` to `App::new`. Then add to `impl App`:

```rust
    pub fn modal_active(&self) -> bool {
        !matches!(self.modal, Modal::None)
    }

    /// Route an input event into the active modal (main-list input is
    /// bypassed while a modal is open).
    pub fn handle_modal_event(&mut self, ev: Event) {
        // Ignore key releases (Windows terminals emit them).
        if let Event::Key(k) = &ev {
            if k.kind == KeyEventKind::Release {
                return;
            }
        }
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        self.modal = match modal {
            Modal::None => Modal::None,
            Modal::Edit(m) => self.edit_modal_event(m, ev),
            Modal::ConfirmDelete(m) => match ev {
                Event::Key(k) if k.code == KeyCode::Char('y') => {
                    let key = m.original.clone().expect("delete only offered when editing");
                    self.delete_item(key);
                    Modal::None
                }
                Event::Key(k) if matches!(k.code, KeyCode::Char('n') | KeyCode::Esc) => {
                    Modal::Edit(m)
                }
                _ => Modal::ConfirmDelete(m),
            },
        };
    }

    fn edit_modal_event(&mut self, mut m: EditModal, ev: Event) -> Modal {
        if let Event::Key(k) = &ev {
            match (k.code, k.modifiers) {
                (KeyCode::Esc, _) => return Modal::None,
                (KeyCode::Tab, _) => {
                    m.cycle_focus();
                    return Modal::Edit(m);
                }
                (KeyCode::Char('s'), mods) if mods.contains(KeyModifiers::CONTROL) => {
                    return self.save_modal(m);
                }
                (KeyCode::Char('d'), mods)
                    if mods.contains(KeyModifiers::CONTROL) && m.original.is_some() =>
                {
                    return Modal::ConfirmDelete(m);
                }
                (KeyCode::Enter, _) if m.focus == EditFocus::Title => {
                    m.focus = EditFocus::Body;
                    m.sync_blocks();
                    return Modal::Edit(m);
                }
                _ => {}
            }
        }
        match m.focus {
            EditFocus::Section => {
                if let Event::Key(k) = &ev {
                    match k.code {
                        KeyCode::Left => {
                            m.choice_idx =
                                (m.choice_idx + m.choices.len() - 1) % m.choices.len();
                        }
                        KeyCode::Right | KeyCode::Char(' ') => {
                            m.choice_idx = (m.choice_idx + 1) % m.choices.len();
                        }
                        _ => {}
                    }
                }
            }
            EditFocus::Title => {
                m.title.input(ev);
            }
            EditFocus::Body => {
                m.body.input(ev);
            }
        }
        Modal::Edit(m)
    }

    fn save_modal(&mut self, m: EditModal) -> Modal {
        let title = m.title_text();
        if title.is_empty() {
            self.status_msg = Some("title required".into());
            return Modal::Edit(m);
        }
        let body = m.body_lines();
        match &m.original {
            Some(key) => {
                let key = key.clone();
                self.with_feed(move |doc| match relocate(doc, &key) {
                    Some(i) => {
                        ops::edit(doc, i, &title, &body);
                        Ok(None)
                    }
                    None => Ok(Some("item changed on disk — edit dropped".into())),
                });
            }
            None => {
                let choice = m.choices[m.choice_idx].clone();
                self.with_feed(move |doc| {
                    match choice {
                        SectionChoice::FirstHuman => ops::add(doc, &title, &body, Zone::Human),
                        SectionChoice::Agent => ops::add(doc, &title, &body, Zone::Agent),
                        SectionChoice::Named(s) => ops::add_in_section(doc, &title, &body, &s)?,
                    }
                    Ok(None)
                });
            }
        }
        Modal::None
    }

    fn delete_item(&mut self, key: ItemKey) {
        self.with_feed(move |doc| match relocate(doc, &key) {
            Some(i) => {
                ops::remove(doc, i);
                Ok(Some("deleted".into()))
            }
            None => Ok(Some("item changed on disk — delete dropped".into())),
        });
    }
```

And extend `apply`'s match — replace the placeholder arm group with:

```rust
            Action::OpenEdit(key) => {
                if let Some(i) = relocate(&self.doc, &key) {
                    if let Node::Item(it) = &self.doc.nodes[i] {
                        self.modal = Modal::Edit(EditModal::edit(key, it));
                    }
                }
            }
            Action::OpenCreate => self.modal = Modal::Edit(EditModal::create(&self.doc)),
            // Wired in Task 10:
            Action::OpenDoneView | Action::OpenFileView | Action::OpenEditor => {}
```

- [ ] **Step 2: Route modal events in `src/tui/mod.rs`.** In `event_loop`, replace the input-handling block with:

```rust
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
```

- [ ] **Step 3: Draw the modal in `src/tui/view.rs`.** Add imports `use crate::tui::app::{EditFocus, Modal};`, `use ratatui::layout::Rect;`, `use ratatui::widgets::{Block, Clear};`. At the end of `draw` (after the status-row render, before the collapsed early-return is unaffected), add `draw_modal(f, app);` and:

```rust
fn draw_modal(f: &mut Frame, app: &App) {
    match &app.modal {
        Modal::None => {}
        Modal::Edit(m) => {
            let area = centered(f.area(), 90, 80);
            f.render_widget(Clear, area);
            let outer = Block::bordered().title(if m.original.is_some() {
                "Edit item"
            } else {
                "New item"
            });
            let inner = outer.inner(area);
            f.render_widget(outer, area);
            let [section, title, body, hints] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .areas(inner);
            if m.original.is_none() {
                let marker = if m.focus == EditFocus::Section { "*" } else { "" };
                f.render_widget(
                    Paragraph::new(format!("Section{marker}: ‹ {} ›", m.choice_label())),
                    section,
                );
            }
            f.render_widget(&m.title, title);
            f.render_widget(&m.body, body);
            let hint = if m.original.is_some() {
                "Tab field · ^S save · ^D delete · Esc cancel"
            } else {
                "Tab field · ^S save · Esc cancel"
            };
            f.render_widget(Paragraph::new(hint), hints);
        }
        Modal::ConfirmDelete(m) => {
            let area = centered(f.area(), 80, 20);
            f.render_widget(Clear, area);
            let outer = Block::bordered().title("Confirm delete");
            let inner = outer.inner(area);
            f.render_widget(outer, area);
            let title = m.original.as_ref().map(|k| k.title.clone()).unwrap_or_default();
            f.render_widget(
                Paragraph::new(format!("Delete \"{title}\"? [y]es / [n]o")),
                inner,
            );
        }
    }
}

fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let [_, mid_v, _] = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .areas(area);
    let [_, mid, _] = Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .areas(mid_v);
    mid
}
```

- [ ] **Step 4: Write the tests** (append inside `src/tui/app.rs`'s `tests` module — behavior-level: drive the modal with events, assert the file)

```rust
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    fn press(app: &mut App, code: KeyCode) {
        app.handle_modal_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    fn press_ctrl(app: &mut App, c: char) {
        app.handle_modal_event(Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::CONTROL,
        )));
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn create_modal_adds_item_to_picked_section() {
        let (mut app, _fake, _dir) =
            app_on_disk("# Feed\n\n- [ ] A\n\n## Later\n\n- [ ] L1\n");
        app.apply(Action::OpenCreate);
        assert!(app.modal_active());
        // Section field focused first; cycle FirstHuman → Later:
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab); // → Title
        type_str(&mut app, "L2");
        press(&mut app, KeyCode::Enter); // → Body
        type_str(&mut app, "ctx");
        press_ctrl(&mut app, 's');
        assert!(!app.modal_active());
        assert!(feed_text(&app).contains("- [ ] L1\n- [ ] L2\n  ctx\n"));
    }

    #[test]
    fn create_modal_agent_section_and_empty_title_rejected() {
        let (mut app, _fake, _dir) = app_on_disk("# Feed\n\n- [ ] A\n");
        app.apply(Action::OpenCreate);
        press_ctrl(&mut app, 's'); // empty title
        assert!(app.modal_active());
        assert_eq!(app.status_msg.as_deref(), Some("title required"));
        // Choices are [FirstHuman, Agent] (no named sections): pick Agent.
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "Agent chore");
        press_ctrl(&mut app, 's');
        assert!(feed_text(&app).contains("## Agent\n\n- [ ] Agent chore\n"));
    }

    #[test]
    fn edit_modal_updates_title_and_body() {
        let (mut app, _fake, _dir) = app_on_disk("- [~] Old @agent(claude:abc)\n  old body\n");
        app.apply(Action::OpenEdit(ItemKey { title: "Old".into(), state: State::InProgress }));
        let Modal::Edit(m) = &app.modal else { panic!("expected edit modal") };
        assert_eq!(m.title_text(), "Old");
        assert_eq!(m.body_lines(), vec!["old body"]);
        type_str(&mut app, "er"); // cursor starts in the title
        press_ctrl(&mut app, 's');
        let text = feed_text(&app);
        assert!(text.contains("Older") && text.contains("@agent(claude:abc)"), "got:\n{text}");
    }

    #[test]
    fn delete_requires_confirm() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] Doomed\n  ctx\n- [ ] Keeper\n");
        app.apply(Action::OpenEdit(ItemKey { title: "Doomed".into(), state: State::Open }));
        press_ctrl(&mut app, 'd');
        assert!(matches!(app.modal, Modal::ConfirmDelete(_)));
        press(&mut app, KeyCode::Char('n')); // back out
        assert!(matches!(app.modal, Modal::Edit(_)));
        press_ctrl(&mut app, 'd');
        press(&mut app, KeyCode::Char('y'));
        assert!(!app.modal_active());
        assert_eq!(feed_text(&app), "- [ ] Keeper\n");
    }

    #[test]
    fn esc_cancels_without_writing() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] A\n");
        app.apply(Action::OpenEdit(ItemKey { title: "A".into(), state: State::Open }));
        type_str(&mut app, "bc");
        press(&mut app, KeyCode::Esc);
        assert!(!app.modal_active());
        assert_eq!(feed_text(&app), "- [ ] A\n");
    }
```

- [ ] **Step 5: Run the tests**

Run: `cargo test tui::app`
Expected: PASS — the 5 new modal tests and all earlier app tests. (`edit_modal_updates_title_and_body` relies on the explicit `CursorMove::End` in `EditModal::edit`.)

- [ ] **Step 6: Full suite + commit**

Run: `cargo test`
Expected: PASS.

```bash
git add src/tui
git commit -m "feat: edit/create modal with section picker and delete confirm"
```

---

### Task 10: Done/file viewer modals + `$EDITOR`

**Files:**
- Modify: `src/tui/app.rs`, `src/tui/view.rs`, `src/tui/mod.rs`

Spec §3: `Done (n)` click → read-only view of the `# Done` archive; file button → read-only plaintext view of the whole feed, with `e` (there, or in the main list) opening `$EDITOR`. The editor spawn suspends the TUI, waits, restores, and reloads (spec §6 documents the whole-file-editor race as accepted for v1).

- [ ] **Step 1: Write the failing tests** (append inside `src/tui/app.rs`'s `tests` module)

```rust
    #[test]
    fn done_view_shows_archive_only() {
        let (mut app, _fake, _dir) = app_on_disk(
            "# Feed\n\n- [ ] Active\n\n# Done\n\n## Feed\n\n- [x] Old @done(2026-09-01)\n\n# Notes\n\nprose\n",
        );
        app.apply(Action::OpenDoneView);
        assert!(matches!(app.modal, Modal::DoneView { .. }));
        let text = app.archive_text();
        assert!(text.contains("- [x] Old @done(2026-09-01)"));
        assert!(!text.contains("Active"));
        assert!(!text.contains("prose")); // stops at the next level-1 heading
    }

    #[test]
    fn done_view_without_archive_says_so() {
        let (app, _fake, _dir) = app_on_disk("- [ ] A\n");
        assert_eq!(app.archive_text(), "No archived items yet.");
    }

    #[test]
    fn file_view_shows_raw_feed_and_e_requests_editor() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] A\n");
        app.editor_cmd = Some("true".into()); // injected — no env mutation in tests
        app.apply(Action::OpenFileView);
        assert!(matches!(app.modal, Modal::FileView { .. }));
        assert_eq!(app.file_text(), "- [ ] A\n");
        press(&mut app, KeyCode::Char('e'));
        assert!(!app.modal_active());
        let expected = app.feed_path.clone();
        assert_eq!(app.take_editor_request(), Some(expected));
        assert_eq!(app.take_editor_request(), None); // one-shot
    }

    #[test]
    fn viewer_esc_closes_and_scrolls() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] A\n");
        app.apply(Action::OpenDoneView);
        press(&mut app, KeyCode::Down);
        assert!(matches!(app.modal, Modal::DoneView { scroll: 1 }));
        press(&mut app, KeyCode::Esc);
        assert!(!app.modal_active());
    }

    #[test]
    fn open_editor_without_editor_configured_reports() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] A\n");
        app.editor_cmd = None; // as if $EDITOR were unset — no env mutation
        app.apply(Action::OpenEditor);
        assert_eq!(app.take_editor_request(), None);
        assert!(app.status_msg.as_deref().unwrap().contains("$EDITOR"));
    }
```

And in `src/tui/mod.rs`'s (new) tests module at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
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
        assert!(std::fs::read_to_string(&target).unwrap().contains("From editor"));
        assert!(super::spawn_editor("/nonexistent-editor-binary", &target).is_err());
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test tui`
Expected: FAIL — `Modal::DoneView`/`FileView`, `archive_text`, `file_text`, `take_editor_request`, `spawn_editor` not defined.

- [ ] **Step 3: Implement in `src/tui/app.rs`.**

Extend the `Modal` enum:

```rust
pub enum Modal {
    None,
    Edit(EditModal),
    ConfirmDelete(EditModal),
    DoneView { scroll: u16 },
    FileView { scroll: u16 },
}
```

Add two fields to `App`:

```rust
    /// $EDITOR captured at startup; injectable in tests (avoids process-global
    /// env mutation racing parallel tests).
    pub editor_cmd: Option<String>,
    editor_request: Option<PathBuf>,
```

initialized in `App::new` as:

```rust
            editor_cmd: std::env::var("EDITOR").ok().filter(|e| !e.is_empty()),
            editor_request: None,
```

In `apply`, replace the `Action::OpenDoneView | Action::OpenFileView | Action::OpenEditor => {}` arm with:

```rust
            Action::OpenDoneView => self.modal = Modal::DoneView { scroll: 0 },
            Action::OpenFileView => self.modal = Modal::FileView { scroll: 0 },
            Action::OpenEditor => self.request_editor(),
```

Add to `impl App`:

```rust
    fn request_editor(&mut self) {
        if self.editor_cmd.is_some() {
            self.editor_request = Some(self.feed_path.clone());
        } else {
            self.status_msg = Some("set $EDITOR to edit the feed externally".into());
        }
    }

    /// One-shot: the event loop takes this, suspends the TUI, and runs $EDITOR.
    pub fn take_editor_request(&mut self) -> Option<PathBuf> {
        self.editor_request.take()
    }

    /// The `# Done` region as rendered markdown (read-only view), cut at the
    /// next level-1 heading so trailing `# Notes`-style sections stay out.
    pub fn archive_text(&self) -> String {
        let Some(start) = self.doc.nodes.iter().position(
            |n| matches!(n, Node::Heading { level: 1, text } if text.eq_ignore_ascii_case("Done")),
        ) else {
            return "No archived items yet.".into();
        };
        let end = self
            .doc
            .nodes
            .iter()
            .enumerate()
            .skip(start + 1)
            .find(|(_, n)| matches!(n, Node::Heading { level: 1, .. }))
            .map(|(j, _)| j)
            .unwrap_or(self.doc.nodes.len());
        crate::feed::write::render(&Document { nodes: self.doc.nodes[start..end].to_vec() })
    }

    /// The feed file verbatim (read-only view).
    pub fn file_text(&self) -> String {
        std::fs::read_to_string(&self.feed_path).unwrap_or_default()
    }
```

In `handle_modal_event`, add the viewer arms to the `match modal`:

```rust
            Modal::DoneView { scroll } => match ev {
                Event::Key(k) if matches!(k.code, KeyCode::Esc | KeyCode::Char('q')) => Modal::None,
                Event::Key(k) if k.code == KeyCode::Down => Modal::DoneView { scroll: scroll + 1 },
                Event::Key(k) if k.code == KeyCode::Up => {
                    Modal::DoneView { scroll: scroll.saturating_sub(1) }
                }
                _ => Modal::DoneView { scroll },
            },
            Modal::FileView { scroll } => match ev {
                Event::Key(k) if matches!(k.code, KeyCode::Esc | KeyCode::Char('q')) => Modal::None,
                Event::Key(k) if k.code == KeyCode::Char('e') => {
                    self.request_editor();
                    Modal::None
                }
                Event::Key(k) if k.code == KeyCode::Down => Modal::FileView { scroll: scroll + 1 },
                Event::Key(k) if k.code == KeyCode::Up => {
                    Modal::FileView { scroll: scroll.saturating_sub(1) }
                }
                _ => Modal::FileView { scroll },
            },
```

- [ ] **Step 4: Draw the viewers in `src/tui/view.rs`** — add two arms to `draw_modal`'s match:

```rust
        Modal::DoneView { scroll } => {
            draw_viewer(f, "Done archive — Esc closes", &app.archive_text(), *scroll)
        }
        Modal::FileView { scroll } => draw_viewer(
            f,
            "feed file — e edits in $EDITOR, Esc closes",
            &app.file_text(),
            *scroll,
        ),
```

and the helper:

```rust
fn draw_viewer(f: &mut Frame, title: &str, text: &str, scroll: u16) {
    let area = centered(f.area(), 90, 80);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(text.to_string())
            .block(Block::bordered().title(title.to_string()))
            .scroll((scroll, 0)),
        area,
    );
}
```

- [ ] **Step 5: Suspend/resume for `$EDITOR` in `src/tui/mod.rs`.** Add at file scope:

```rust
/// Run $EDITOR (may carry args, e.g. "code -w") on the feed file.
pub fn spawn_editor(editor: &str, path: &std::path::Path) -> Result<()> {
    let mut parts = editor.split_whitespace();
    let bin = parts.next().ok_or_else(|| anyhow::anyhow!("$EDITOR is empty"))?;
    let status = std::process::Command::new(bin).args(parts).arg(path).status()?;
    if !status.success() {
        anyhow::bail!("editor exited with {status}");
    }
    Ok(())
}
```

and in `event_loop`, directly after the `while let Ok(ev) = rx.try_recv()` drain, add:

```rust
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
```

- [ ] **Step 6: Run the tests, then the full suite**

Run: `cargo test tui` then `cargo test`
Expected: PASS — 6 new tests (5 app + 1 mod), everything else green.

- [ ] **Step 7: Commit**

```bash
git add src/tui
git commit -m "feat: done/file viewer modals and \$EDITOR suspend-resume"
```

---

### Task 11: File watcher → auto-refresh

**Files:**
- Create: `src/tui/watch.rs`
- Modify: `src/tui/mod.rs` (add `pub mod watch;`, wire the watcher into `run`)

Spec §3: refresh is automatic, no refresh key. Decision 1: watch the **parent directory** (non-recursive), filtered to the feed's filename — `save_atomic` replaces the file by rename, which would orphan a per-file watch. Any matching event sends `AppEvent::FeedChanged`; the event loop drains the channel, so bursts coalesce into one reload (reloads are idempotent, so reacting to our own saves is harmless).

- [ ] **Step 1: Write the failing test** (bottom of the new `src/tui/watch.rs`)

```rust
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
        std::fs::write(dir.path().join("other.txt"), "noise").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test tui::watch`
Expected: FAIL — `spawn` not defined.

- [ ] **Step 3: Implement** (top of `src/tui/watch.rs`)

```rust
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
            if ev.paths.iter().any(|p| p.file_name() == Some(file_name.as_os_str())) {
                // Receiver gone = app quit; nothing to do.
                let _ = tx.send(AppEvent::FeedChanged);
            }
        }
    })?;
    watcher.watch(&dir, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}
```

Add `pub mod watch;` to `src/tui/mod.rs`, and wire it into `run` — replace the channel/app setup lines with:

```rust
    let (tx, rx) = mpsc::channel::<AppEvent>();
    let _watcher = watch::spawn(&feed_path, tx.clone())
        .map_err(|e| anyhow::anyhow!("cannot watch {}: {e}", feed_path.display()))?;
    let mut app = App::new(feed_path, cfg, Box::new(NoHerdr));
    app.reload();
```

(`_watcher` must stay bound for the lifetime of `run` — dropping it stops events.)

- [ ] **Step 4: Run the tests**

Run: `cargo test tui::watch`
Expected: PASS — 2 tests. (macOS FSEvents can take ~1s to deliver; the 5s timeout covers it. If the sibling-file test flakes because the OS coalesces directory events oddly, it may be relaxed to assert that no event *for the feed* arrives — but try as written first.)

- [ ] **Step 5: Full suite + commit**

Run: `cargo test`
Expected: PASS.

```bash
git add src/tui
git commit -m "feat: auto-refresh via directory watch surviving atomic renames"
```

---

### Task 12: Live socket client + event subscription

**Files:**
- Modify: `src/tui/socket.rs` (real client + event thread), `src/tui/mod.rs` (use them)

The real NDJSON client (spec §7): one fresh connection per request (`{"id":...,"method":...,"params":...}` + one response line), plus a long-lived subscription connection streaming events. Any pane event triggers a full `agent.list` resync — statuses are tiny, and this sidesteps per-event-type diffing (the research doc's recommended resync-on-reconnect pattern, applied to every event). The subscriber thread reconnects every 5s; while down, the app's statuses are cleared (glyphs hidden — the degraded mode).

The resume flow's verbs were verified by the lead against live herdr 0.9.0 (`herdr api schema --json`): **no method spawns a command directly** — `tab.create` (TabCreateParams: workspace_id/cwd/env/label/focus) opens a shell pane, then `agent.start` (AgentStartParams: {name, kind, pane_id, args[], timeout_ms}) starts the agent there with `--resume <session-id>` riding in `args`. CLI equivalents: `herdr tab create --label <t> --focus`, then `herdr agent start <name> --kind claude --pane <PANE_ID> -- --resume <id>`. A successful `agent.start` means herdr *detected* the agent, so the item's `@agent` link is live again with proper status on the next resync. The client implements the trait's `create_tab`/`agent_start` primitives over the socket; `open_resume_tab` is the trait's provided composition (Task 3).

- [ ] **Step 1: Write the failing tests** (append inside `src/tui/socket.rs`'s `tests` module)

```rust
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::os::unix::net::UnixListener;

    /// Sequential fake herdr: for each accepted connection, read one request
    /// line (recorded for assertions), write the scripted response lines, close.
    fn fake_server(
        scripts: Vec<Vec<String>>,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        std::thread::spawn(move || {
            for script in scripts {
                let Ok((stream, _)) = listener.accept() else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                seen2.lock().unwrap().push(line.trim().to_string());
                let mut w = stream;
                for l in script {
                    let _ = writeln!(w, "{l}");
                }
            }
        });
        (dir, path, seen)
    }

    #[test]
    fn live_client_lists_agents_and_surfaces_errors() {
        let ok = serde_json::json!({"id": "feedr", "result": {"agents": [
            {"pane_id": "w1:p7", "agent": "claude", "agent_status": "blocked",
             "agent_session": {"value": "abc"}}]}})
        .to_string();
        let err =
            r#"{"id":"feedr","error":{"code":"not_found","message":"pane not found"}}"#.to_string();
        let (_dir, path, _seen) = fake_server(vec![vec![ok], vec![err]]);
        let mut c = UnixSocketClient { socket_path: path };
        let agents = c.list_agents().unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].status, AgentStatus::Blocked);
        assert_eq!(agents[0].session_id, "abc");
        let e = c.focus_agent("w1:p9").unwrap_err();
        assert!(e.to_string().contains("pane not found"), "got: {e}");
    }

    #[test]
    fn absent_socket_fails_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = UnixSocketClient { socket_path: dir.path().join("nope.sock") };
        let e = c.list_agents().unwrap_err();
        assert!(e.to_string().contains("herdr socket unavailable"), "got: {e}");
    }

    #[test]
    fn live_resume_flow_creates_tab_then_starts_agent() {
        let tab = r#"{"id":"feedr","result":{"tab_id":"w1:t9","pane_id":"w1:p9"}}"#.to_string();
        let ok = r#"{"id":"feedr","result":{}}"#.to_string();
        let (_dir, path, seen) = fake_server(vec![vec![tab], vec![ok]]);
        let mut c = UnixSocketClient { socket_path: path };
        c.open_resume_tab("claude", "abc-123").unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "wire: {seen:?}");
        assert!(seen[0].contains("\"method\":\"tab.create\""), "got: {}", seen[0]);
        assert!(seen[0].contains("\"label\":\"resume claude\""));
        assert!(seen[1].contains("\"method\":\"agent.start\""), "got: {}", seen[1]);
        assert!(seen[1].contains("\"kind\":\"claude\""));
        assert!(seen[1].contains("\"pane_id\":\"w1:p9\""));
        assert!(seen[1].contains("--resume"));
    }

    #[test]
    fn create_tab_falls_back_to_pane_list_for_pane_id() {
        // A tab.create response that names only the tab: resolve the pane
        // via pane.list filtered to the new tab_id.
        let tab = r#"{"id":"feedr","result":{"tab_id":"w1:t9"}}"#.to_string();
        let panes = r#"{"id":"feedr","result":{"panes":[
            {"pane_id":"w1:p2","tab_id":"w1:t1"},
            {"pane_id":"w1:p9","tab_id":"w1:t9"}]}}"#
            .to_string();
        let (_dir, path, _seen) = fake_server(vec![vec![tab], vec![panes]]);
        let mut c = UnixSocketClient { socket_path: path };
        assert_eq!(c.create_tab("resume claude").unwrap(), "w1:p9");
    }

    #[test]
    fn subscribe_loop_resyncs_on_events() {
        let list = serde_json::json!({"id": "feedr", "result": {"agents": [
            {"pane_id": "w1:p7", "agent": "claude", "agent_status": "working",
             "agent_session": {"value": "abc"}}]}})
        .to_string();
        let ack = r#"{"id":"sub","result":{}}"#.to_string();
        let event = r#"{"type":"pane_agent_status_changed","pane_id":"w1:p7","agent_status":"done"}"#
            .to_string();
        let (_dir, path, _seen) = fake_server(vec![
            vec![list.clone()], // initial resync
            vec![ack, event],   // subscription: ack (skipped), one event, EOF
            vec![list],         // resync triggered by the event
        ]);
        let (tx, rx) = std::sync::mpsc::channel();
        subscribe_loop(&path, &tx).unwrap(); // returns at EOF
        let mut agent_batches = 0;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, crate::tui::AppEvent::Agents(_)) {
                agent_batches += 1;
            }
        }
        assert_eq!(agent_batches, 2);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test tui::socket`
Expected: FAIL — `UnixSocketClient`, `subscribe_loop` not defined.

- [ ] **Step 3: Implement in `src/tui/socket.rs`.** Add imports at the top:

```rust
use anyhow::Context;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
```

Then add below the trait:

```rust
pub fn socket_path_from_env() -> PathBuf {
    // Injected into plugin processes by herdr; default-session path otherwise
    // (research doc: ~/.config/herdr/herdr.sock).
    std::env::var_os("HERDR_SOCKET_PATH")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("herdr/herdr.sock")
        })
}

pub fn herdr_bin_from_env() -> String {
    std::env::var("HERDR_BIN_PATH")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "herdr".into())
}

/// First string value under `key` anywhere in a JSON value. Response
/// envelopes vary between the socket and the CLI wrapper; the research doc
/// pins fields, not nesting — so search structurally.
pub(crate) fn find_string_field(v: &Value, key: &str) -> Option<String> {
    match v {
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get(key) {
                return Some(s.clone());
            }
            map.values().find_map(|v| find_string_field(v, key))
        }
        Value::Array(a) => a.iter().find_map(|v| find_string_field(v, key)),
        _ => None,
    }
}

pub struct UnixSocketClient {
    pub socket_path: PathBuf,
}

impl UnixSocketClient {
    pub fn from_env() -> Self {
        UnixSocketClient { socket_path: socket_path_from_env() }
    }
}

/// One NDJSON request/response round-trip on a fresh connection.
fn request(path: &Path, method: &str, params: serde_json::Value) -> anyhow::Result<Value> {
    let mut stream = UnixStream::connect(path)
        .with_context(|| format!("herdr socket unavailable at {}", path.display()))?;
    let req = serde_json::json!({"id": "feedr", "method": method, "params": params});
    stream.write_all(format!("{req}\n").as_bytes())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let resp: Value = serde_json::from_str(&line).context("bad NDJSON from herdr")?;
    if let Some(err) = resp.get("error") {
        anyhow::bail!(
            "herdr: {}",
            err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error")
        );
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

impl Herdr for UnixSocketClient {
    fn list_agents(&mut self) -> Result<Vec<AgentInfo>> {
        Ok(parse_agent_list(&request(
            &self.socket_path,
            "agent.list",
            serde_json::json!({}),
        )?))
    }
    fn focus_agent(&mut self, target: &str) -> Result<()> {
        request(&self.socket_path, "agent.focus", serde_json::json!({"target": target}))?;
        Ok(())
    }
    fn focus_pane(&mut self, pane_id: &str) -> Result<()> {
        request(&self.socket_path, "pane.focus", serde_json::json!({"pane_id": pane_id}))?;
        Ok(())
    }
    fn create_tab(&mut self, label: &str) -> Result<String> {
        // tab.create spawns a shell pane (TabCreateParams has no command).
        // Focus is intended here: resume is user-initiated navigation.
        let result = request(
            &self.socket_path,
            "tab.create",
            serde_json::json!({"label": label, "focus": true}),
        )?;
        if let Some(pane_id) = find_string_field(&result, "pane_id") {
            return Ok(pane_id);
        }
        // Response named only the tab: resolve its pane via pane.list.
        let tab_id = find_string_field(&result, "tab_id")
            .context("no tab_id in tab.create response")?;
        let panes = request(&self.socket_path, "pane.list", serde_json::json!({}))?;
        result_entries(&panes)
            .into_iter()
            .find(|e| e.get("tab_id").and_then(|t| t.as_str()) == Some(tab_id.as_str()))
            .and_then(|e| Some(e.get("pane_id")?.as_str()?.to_string()))
            .context("created tab has no pane")
    }
    fn agent_start(&mut self, name: &str, kind: &str, pane_id: &str, args: &[String]) -> Result<()> {
        request(
            &self.socket_path,
            "agent.start",
            serde_json::json!({"name": name, "kind": kind, "pane_id": pane_id, "args": args}),
        )?;
        Ok(())
    }
    // open_resume_tab: the trait's provided create_tab + agent_start
    // composition (Task 3) — no override needed.
}

/// Background subscriber: resync, stream events, resync again on every pane
/// event; reconnect with a 5s backoff. Exits when the app drops the receiver.
pub fn spawn_event_thread(socket_path: PathBuf, tx: mpsc::Sender<crate::tui::AppEvent>) {
    std::thread::spawn(move || loop {
        let _ = subscribe_loop(&socket_path, &tx);
        if tx.send(crate::tui::AppEvent::SocketDown).is_err() {
            return; // app gone
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
    });
}

/// Blocks streaming events until the connection drops (or the app goes away).
/// Subscription types per the research doc's recommended wiring.
pub fn subscribe_loop(path: &Path, tx: &mpsc::Sender<crate::tui::AppEvent>) -> Result<()> {
    let agents = parse_agent_list(&request(path, "agent.list", serde_json::json!({}))?);
    tx.send(crate::tui::AppEvent::Agents(agents))
        .map_err(|_| anyhow::anyhow!("app gone"))?;
    let mut stream = UnixStream::connect(path)?;
    let sub = serde_json::json!({"id": "sub", "method": "events.subscribe", "params": {"subscriptions": [
        {"type": "pane.agent_status_changed"},
        {"type": "pane.created"},
        {"type": "pane.closed"},
        {"type": "pane.agent_detected"}
    ]}});
    stream.write_all(format!("{sub}\n").as_bytes())?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        // Pushed events carry "type"; the subscribe ack carries "id" — skip it.
        if v.get("type").is_some() {
            let agents = parse_agent_list(&request(path, "agent.list", serde_json::json!({}))?);
            tx.send(crate::tui::AppEvent::Agents(agents))
                .map_err(|_| anyhow::anyhow!("app gone"))?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Use the live client in `src/tui/mod.rs`.** Delete the `NoHerdr` struct and its impl entirely, and in `run` replace the app construction with:

```rust
    let (tx, rx) = mpsc::channel::<AppEvent>();
    let _watcher = watch::spawn(&feed_path, tx.clone())
        .map_err(|e| anyhow::anyhow!("cannot watch {}: {e}", feed_path.display()))?;
    socket::spawn_event_thread(socket::socket_path_from_env(), tx.clone());
    let mut app = App::new(feed_path, cfg, Box::new(socket::UnixSocketClient::from_env()));
    app.reload();
```

(When herdr isn't running: the subscriber fails fast, sends `SocketDown`, and retries every 5s — statuses stay empty, glyphs hidden, and an `@agent` click surfaces "herdr socket unavailable … resume manually: claude --resume <id>" in the status line. Exactly the spec'd degradation.)

- [ ] **Step 5: Note on verbs (already verified — do not re-litigate).** `tab.create` and `agent.start` (and their param shapes) were verified by the lead against live herdr 0.9.0 via `herdr api schema --json`; there is **no** method that spawns a command directly (`tab.create` and `pane.split` both spawn plain shells). Any failure in the flow (herdr absent, method error, undetected agent) surfaces as `Err` from the trait and degrades to the status-line manual-resume hint (Task 7) — no further fallback work needed here.

- [ ] **Step 6: Run the tests, then the full suite**

Run: `cargo test tui::socket` then `cargo test`
Expected: PASS — 5 new socket tests, everything green.

- [ ] **Step 7: Commit**

```bash
git add src/tui
git commit -m "feat: live herdr socket client with event subscription and resync"
```

---

### Task 13: Dock launcher (`feedr sidebar --dock`) + collapse resize

**Files:**
- Create: `src/tui/dock.rs`
- Modify: `src/tui/mod.rs` (add `pub mod dock;`, set the pane-title marker), `src/tui/app.rs` (collapse resize), `src/cli.rs` (`--dock` flag), `tests/cli.rs`

The beads dock pattern from `docs/research/pane-placements.md`: idempotent open-or-focus; open = split right → swap-walk to the left edge (`pane neighbor` + `pane swap`, ≤6 steps) → shrink with a relative `pane resize` delta. The running sidebar sets its terminal title to a marker so the launcher can find it in `pane list` output (beads' OSC-marker discovery). Docking is herdr-side: the TUI itself only draws.

Lead-verified against herdr 0.9.0: `pane.split` takes **no command** (PaneSplitParams: direction/ratio/cwd/env/focus/right_click/target_pane_id/workspace_id — it spawns the default shell). The interim standalone dock is therefore: `pane split --direction right` (returns the new pane id) → `pane send-text` (PaneSendTextParams: {pane_id, text}) typing `exec feedr sidebar\n` into that shell (`exec` so the sidebar replaces the shell and owns the pane) → swap-walk → resize. Plan 3's plugin manifest replaces the open+exec pair with `plugin.pane.open` (PluginPaneOpenParams, with placement); the `Runner` seam makes that a small, contained change.

- [ ] **Step 1: Write the failing tests** (bottom of the new `src/tui/dock.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Side, SidebarConfig};
    use crate::tui::socket::FakeHerdr;

    fn cfg(side: Side) -> SidebarConfig {
        SidebarConfig { side, width: 0.18, auto_dock: false }
    }

    struct FakeRunner {
        calls: Vec<String>,
        responses: std::collections::VecDeque<anyhow::Result<String>>,
    }

    impl FakeRunner {
        fn new(responses: Vec<anyhow::Result<String>>) -> Self {
            FakeRunner { calls: Vec::new(), responses: responses.into() }
        }
    }

    impl Runner for FakeRunner {
        fn run(&mut self, args: &[&str]) -> anyhow::Result<String> {
            self.calls.push(args.join(" "));
            self.responses.pop_front().unwrap_or(Ok(String::new()))
        }
    }

    #[test]
    fn dock_focuses_existing_sidebar_pane() {
        let list = r#"{"id":"cli:pane:list","result":{"panes":[
            {"pane_id":"w1:p2","terminal_title_stripped":"vim"},
            {"pane_id":"w1:p3","terminal_title_stripped":"feedr-sidebar"}]}}"#;
        let mut runner = FakeRunner::new(vec![Ok(list.into())]);
        let mut herdr = FakeHerdr::default();
        let msg = dock(&mut runner, &mut herdr, &cfg(Side::Left)).unwrap();
        assert!(msg.contains("focused"));
        assert_eq!(herdr.log.borrow().as_slice(), ["focus_pane w1:p3"]);
        assert_eq!(runner.calls, ["pane list"]); // idempotent: nothing opened
    }

    #[test]
    fn dock_opens_execs_sidebar_swap_walks_left_and_resizes() {
        let empty = r#"{"id":"cli:pane:list","result":{"panes":[]}}"#;
        let split = r#"{"id":"cli:pane:split","result":{"pane_id":"w1:p9"}}"#;
        let mut runner = FakeRunner::new(vec![
            Ok(empty.into()),
            Ok(split.into()),
            Ok(String::new()),                   // send-text
            Ok(String::new()),                   // neighbor 1: one exists
            Ok(String::new()),                   // swap 1
            Err(anyhow::anyhow!("no neighbor")), // neighbor 2: at the edge
        ]);
        let mut herdr = FakeHerdr::default();
        let msg = dock(&mut runner, &mut herdr, &cfg(Side::Left)).unwrap();
        assert!(msg.contains("opened sidebar pane w1:p9"));
        assert_eq!(
            runner.calls,
            [
                "pane list",
                "pane split --direction right",
                "pane send-text --pane w1:p9 --text exec feedr sidebar\n",
                "pane neighbor --direction left --pane w1:p9",
                "pane swap --direction left --pane w1:p9",
                "pane neighbor --direction left --pane w1:p9",
                "pane resize --direction left --amount 0.18 --pane w1:p9",
            ]
        );
        assert!(herdr.log.borrow().is_empty());
    }

    #[test]
    fn dock_right_side_skips_swap_walk() {
        let empty = r#"{"id":"cli:pane:list","result":{"panes":[]}}"#;
        let split = r#"{"id":"cli:pane:split","result":{"pane_id":"w1:p9"}}"#;
        let mut runner = FakeRunner::new(vec![Ok(empty.into()), Ok(split.into())]);
        let mut herdr = FakeHerdr::default();
        dock(&mut runner, &mut herdr, &cfg(Side::Right)).unwrap();
        assert_eq!(
            runner.calls,
            [
                "pane list",
                "pane split --direction right",
                "pane send-text --pane w1:p9 --text exec feedr sidebar\n",
                "pane resize --direction right --amount 0.18 --pane w1:p9",
            ]
        );
    }

    #[test]
    fn collapse_resize_uses_inverse_deltas() {
        let mut r = FakeRunner::new(vec![]);
        resize_for_collapse(&mut r, &cfg(Side::Left), true, "w1:p3").unwrap();
        resize_for_collapse(&mut r, &cfg(Side::Left), false, "w1:p3").unwrap();
        resize_for_collapse(&mut r, &cfg(Side::Right), true, "w1:p3").unwrap();
        assert_eq!(
            r.calls,
            [
                "pane resize --direction left --amount 0.18 --pane w1:p3",
                "pane resize --direction right --amount 0.18 --pane w1:p3",
                "pane resize --direction right --amount 0.18 --pane w1:p3",
            ]
        );
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test tui::dock`
Expected: FAIL — nothing defined.

- [ ] **Step 3: Implement** (top of `src/tui/dock.rs`)

```rust
use crate::config::{Side, SidebarConfig};
use crate::tui::socket::{result_entries, Herdr};
use anyhow::{Context, Result};
use serde_json::Value;

/// Terminal title the running sidebar sets so the launcher can find its pane
/// in `pane list` output (the herdr-beads OSC-marker discovery pattern).
pub const PANE_TITLE_MARKER: &str = "feedr-sidebar";

/// Shell-out seam for the herdr CLI so tests fake it.
pub trait Runner {
    /// Run `herdr <args>`, returning stdout; Err on spawn failure or non-zero exit.
    fn run(&mut self, args: &[&str]) -> Result<String>;
}

pub struct HerdrCli {
    pub bin: String,
}

impl HerdrCli {
    pub fn from_env() -> Self {
        HerdrCli { bin: crate::tui::socket::herdr_bin_from_env() }
    }
}

impl Runner for HerdrCli {
    fn run(&mut self, args: &[&str]) -> Result<String> {
        let out = std::process::Command::new(&self.bin)
            .args(args)
            .output()
            .with_context(|| format!("cannot run {}", self.bin))?;
        if !out.status.success() {
            anyhow::bail!(
                "herdr {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Idempotent dock launcher (spec §3, beads pattern): sidebar pane already
/// open → focus it (reopen ≡ focus, which also recovers after a herdr server
/// restart); otherwise split → swap-walk to the edge → shrink.
pub fn dock(runner: &mut dyn Runner, herdr: &mut dyn Herdr, cfg: &SidebarConfig) -> Result<String> {
    let list = runner.run(&["pane", "list"])?;
    if let Some(pane_id) = find_sidebar_pane(&list) {
        herdr.focus_pane(&pane_id)?;
        return Ok(format!("focused existing sidebar pane {pane_id}"));
    }
    // pane.split spawns a plain shell (no command param in 0.9.0) and splits
    // only right/down: split right, exec the sidebar into the new shell,
    // then swap-walk left.
    let out = runner.run(&["pane", "split", "--direction", "right"])?;
    let pane_id = crate::tui::socket::find_string_field(
        &serde_json::from_str::<Value>(&out).unwrap_or(Value::Null),
        "pane_id",
    )
    .context("no pane_id in `herdr pane split` output")?;
    runner.run(&["pane", "send-text", "--pane", &pane_id, "--text", "exec feedr sidebar\n"])?;
    if cfg.side == Side::Left {
        for _ in 0..6 {
            if runner
                .run(&["pane", "neighbor", "--direction", "left", "--pane", &pane_id])
                .is_err()
            {
                break; // at the left edge
            }
            if runner
                .run(&["pane", "swap", "--direction", "left", "--pane", &pane_id])
                .is_err()
            {
                break;
            }
        }
    }
    let dir = match cfg.side {
        Side::Left => "left",
        Side::Right => "right",
    };
    let amount = format!("{}", cfg.width);
    runner.run(&["pane", "resize", "--direction", dir, "--amount", &amount, "--pane", &pane_id])?;
    Ok(format!("opened sidebar pane {pane_id}"))
}

fn find_sidebar_pane(pane_list_stdout: &str) -> Option<String> {
    let v: Value = serde_json::from_str(pane_list_stdout).ok()?;
    let result = v.get("result").unwrap_or(&v);
    result_entries(result).into_iter().find_map(|e| {
        let title = e
            .get("terminal_title_stripped")
            .or_else(|| e.get("terminal_title"))?
            .as_str()?;
        if title.contains(PANE_TITLE_MARKER) {
            Some(e.get("pane_id")?.as_str()?.to_string())
        } else {
            None
        }
    })
}

/// Best-effort pane resize behind the «/» toggle. Plugin v1 resize amounts
/// are relative, so "restore previous width" is the inverse delta —
/// approximate by design (research doc). Callers ignore errors (the rendered
/// rail still collapses without herdr).
pub fn resize_for_collapse(
    runner: &mut dyn Runner,
    cfg: &SidebarConfig,
    collapsed: bool,
    pane_id: &str,
) -> Result<()> {
    // A left-edge pane's movable edge is its right one: shrink = resize left.
    let dir = match (cfg.side, collapsed) {
        (Side::Left, true) | (Side::Right, false) => "left",
        _ => "right",
    };
    let amount = format!("{}", cfg.width);
    runner.run(&["pane", "resize", "--direction", dir, "--amount", &amount, "--pane", pane_id])?;
    Ok(())
}
```

Add `pub mod dock;` to `src/tui/mod.rs`.

- [ ] **Step 4: Wire everything up.**

In `src/cli.rs`, change the `Sidebar` variant to:

```rust
    /// Launch the sidebar TUI in this terminal
    Sidebar {
        /// Dock a sidebar pane into herdr (idempotent open-or-focus), then exit
        #[arg(long)]
        dock: bool,
    },
```

and the dispatch block to:

```rust
    if let Cmd::Sidebar { dock } = &cli.command {
        let cfg = config::load_sidebar_config(&config::default_config_dir());
        if *dock {
            let mut runner = crate::tui::dock::HerdrCli::from_env();
            let mut client = crate::tui::socket::UnixSocketClient::from_env();
            let msg = crate::tui::dock::dock(&mut runner, &mut client, &cfg)?;
            println!("{msg}");
            return Ok(());
        }
        return crate::tui::run(path, cfg);
    }
```

In `src/tui/mod.rs`'s `run`, right after `let mut terminal = ratatui::init();`, add (never steal focus — the marker is title-only):

```rust
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle(dock::PANE_TITLE_MARKER)
    );
```

In `src/tui/app.rs`, add a field to `App` (same injection pattern as `editor_cmd`, so tests never touch process-global env):

```rust
    /// Our own herdr pane id (env HERDR_PANE_ID), when running inside herdr.
    pub herdr_pane_id: Option<String>,
```

initialized in `App::new` as:

```rust
            herdr_pane_id: std::env::var("HERDR_PANE_ID").ok().filter(|p| !p.is_empty()),
```

and change `apply`'s `Action::ToggleCollapse` arm to:

```rust
            Action::ToggleCollapse => {
                self.collapsed = !self.collapsed;
                // Best-effort real pane resize; outside herdr there is no
                // pane id and the rendered rail alone carries the collapse.
                if let Some(pane) = self.herdr_pane_id.clone() {
                    let mut runner = crate::tui::dock::HerdrCli::from_env();
                    let _ = crate::tui::dock::resize_for_collapse(
                        &mut runner,
                        &self.cfg,
                        self.collapsed,
                        &pane,
                    );
                }
            }
```

(Existing app tests still pass: `App::new` in a normal test environment finds no `HERDR_PANE_ID`, so nothing is spawned.)

In `tests/cli.rs`, extend the sidebar test:

```rust
#[test]
fn sidebar_subcommand_is_wired() {
    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.args(["sidebar", "--help"])
        .assert()
        .success()
        .stdout(contains("--dock"));
}
```

- [ ] **Step 5: Verify the send-text CLI spelling** (bounded check — the socket method `pane.send_text` / PaneSendTextParams {pane_id, text} is lead-verified; only the CLI wrapper's subcommand/flag names need confirming):

Run: `herdr pane --help 2>/dev/null || true`

- Confirm the subcommand (`send-text` vs `send-keys` — the schema carries both send-text and send-keys param types) and its flag names (`--pane`/`--text` or positional). If they differ from `pane send-text --pane <id> --text <text>`, adjust ONLY the send-text invocation in `dock()` and the expected strings in the two dock tests.
- If herdr isn't installed here, leave as written — `--dock` is exercised by its fake-runner tests, and Plan 3's packaging pass (which replaces the split+exec pair with `plugin pane open`) does the real-herdr verification.

- [ ] **Step 6: Run the tests, then the full suite**

Run: `cargo test tui::dock` then `cargo test`
Expected: PASS — 4 new dock tests, everything green.

- [ ] **Step 7: Commit**

```bash
git add src/tui src/cli.rs tests/cli.rs
git commit -m "feat: idempotent --dock launcher (beads split/swap/resize pattern)"
```

---

### Task 14: Wrap-up

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Lint + full suite**

Run: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`
Expected: clean. Fix anything that isn't (fmt/clippy autofixes are fine; do not silence lints with `allow` without a comment saying why).

- [ ] **Step 2: Manual smoke test** (a real terminal, outside herdr)

```bash
printf '# Feed\n\n- [ ] Try the sidebar\n' > /tmp/smoke-feed.md
FEEDR_FEED=/tmp/smoke-feed.md cargo run -- sidebar
```

Verify: checkbox click advances; title click opens the modal and ^S saves; `a` creates; broom (`sweep`) works after ticking an item and `Done (1)` shows the archive; `«` collapses to `»` and a click restores; editing /tmp/smoke-feed.md from another terminal repaints the sidebar within ~1s; `q` quits with the terminal restored. (Without herdr running, `@agent` glyphs are absent — expected.)

- [ ] **Step 3: Update the README status line** to:

```markdown
**Status: core + CLI + sidebar TUI implemented** (`cargo install --path .` → `feedr sidebar`, or `feedr sidebar --dock` inside herdr). Herdr plugin packaging and the agent skill are next; the v1 spec is in [docs/SPEC.md](docs/SPEC.md).
```

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: README status — sidebar TUI implemented"
```

---

## Self-review notes

- **Spec §3 coverage, line by line:** rendering (sections/glyphs/ellipsis/agent sub-line with live glyph) → Tasks 4–5; checkbox advance incl. `[?]`-click-accepts and never-produces-`[?]` → Task 7 (tested explicitly); title-click edit modal + `+`/`a` create + delete-with-confirm → Task 9; `e` → `$EDITOR` → Task 10; `@agent` click focus/resume/degrade → Tasks 7 & 12; `«`/`»` collapse → Tasks 5/7 (render+flag) & 13 (real resize); broom sweep → Task 7; bottom-pinned `Done (n)` + archive modal → Tasks 5 & 10; file button (internal view / external editor) → Task 10; auto-refresh (file-watch + socket events), no refresh key → Tasks 11–12; never steals focus → the sidebar only ever draws in its own pane, and the `--dock` marker write is title-only. `toggle-feedr` plugin action and `tab.created` auto-dock are manifest-level → Plan 3 (config's `auto_dock` is parsed here so Plan 3 needs no config change).
- **Spec §6:** every TUI write path funnels through `App::with_feed` (fresh read → one op → `save_atomic` → reload); `relocate` misses drop the action with a message; the `$EDITOR` race is accepted and commented.
- **Spec §7:** socket supplies exactly statuses + join key (`agent_session.value` ↔ `@agent` id) + focus; no transcript replay.
- **Spec §2/§2a:** all state changes call `ops::set_state(..., Authority::Human)`; sweep calls `ops::sweep`; the plan adds `edit`/`remove`/`add_in_section` but re-specifies nothing existing (and `agent_section_end` is refactored onto the new generalized helper, covered by existing tests).
- **Type consistency check:** `ItemKey{title,state}`, `Row::{Section,Item,AgentLine,Add}`, `Action` variants, `Modal::{None,Edit,ConfirmDelete,DoneView,FileView}`, `Herdr::{list_agents,focus_agent,focus_pane,create_tab,agent_start}` + provided `open_resume_tab` (composition defined once in Task 3, exercised via the fake in Tasks 3/7 and over the wire in Task 12), `AgentInfo{pane_id,kind,session_id,status}`, `SidebarConfig{side,width,auto_dock}`, `resume_args`/`resume_hint` — used identically across Tasks 3–13; toolbar column spans in Task 6 match Task 5's `TOOLBAR` constant (noted in both); `find_string_field` lives in `socket.rs` (Task 12) and is shared by `dock.rs` (Task 13); `UnixSocketClient{socket_path}` has no herdr-bin field (only `dock::HerdrCli` shells the CLI, via `socket::herdr_bin_from_env`).
- **Placeholder scan:** the two `Action::Open*` stub arms in Task 7 are explicitly labeled with the task that replaces them (9 and 10) and both replacements carry full code; both herdr integration flows (resume: `tab.create`+`agent.start`; dock: `pane split`+`send-text`) are lead-verified against live herdr 0.9.0, with one bounded check remaining (Task 13's CLI send-text spelling) and coded fallbacks — no "TBD" anywhere.
- **Known deliberate gaps (v1 — lead-settled):** modal is keyboard-driven (no mouse targets inside it); keyboard-only list navigation deferred to post-v1 (mouse + `a`/`e`/`q` per spec's interaction list — lead decision (f)); duplicate-titled same-state items relocate to the first match; `feedr add --section` on the CLI still bails (the modal is the section-picker path — completing the flag is a trivial follow-up via `add_in_section` if wanted).







