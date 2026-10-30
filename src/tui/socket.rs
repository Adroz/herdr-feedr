use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// Per-request timeout: `request()` runs on the TUI main thread, so a
/// stalled herdr must not freeze the app — it surfaces as an `Err` through
/// the existing error paths instead.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// Process-wide counter for request ids, so each `request()` call can
/// verify the response it reads back is actually the one it sent (and not,
/// say, a stale reply left over on a reused/misbehaving connection).
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(0);

fn next_request_id() -> String {
    format!("feedr-{}", NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed))
}

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
                    v.get("agent_status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown"),
                ),
            })
        })
        .collect()
}

/// One `pane.list` entry: where a pane lives and what it shows. herdr's
/// `PaneInfo` (0.9.3) carries no plugin id or zoom flag; zoomed and overlay
/// panes are listed like any other, and a feedr sidebar is recognised by the
/// title it sets on itself (`dock::PANE_TITLE_MARKER`). Popups are never
/// listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneEntry {
    pub pane_id: String,
    pub tab_id: String,
    pub title: Option<String>,
}

impl PaneEntry {
    /// A feedr sidebar: never company for auto-close.
    pub fn is_feedr(&self) -> bool {
        self.title.as_deref() == Some(crate::tui::dock::PANE_TITLE_MARKER)
    }
}

/// Our pane's current tab in a `pane.list` answer. `HERDR_TAB_ID` is only
/// a hint fixed at spawn (`pane.move` changes the tab), so resolve it live.
/// `None` when our id no longer resolves (e.g. moved across workspaces).
pub fn own_tab<'a>(panes: &'a [PaneEntry], own_pane: &str) -> Option<&'a str> {
    panes
        .iter()
        .find(|p| p.pane_id == own_pane)
        .map(|p| p.tab_id.as_str())
}

pub fn parse_pane_list(result: &Value) -> Vec<PaneEntry> {
    result_entries(result)
        .into_iter()
        .filter_map(|v| {
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
            Some(PaneEntry {
                pane_id: s("pane_id")?,
                tab_id: s("tab_id")?,
                title: s("terminal_title_stripped").or_else(|| s("terminal_title")),
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
    /// `pane.zoom {pane_id, mode}` — zoom (`on`) or unzoom (`off`) a pane to
    /// fill its whole tab (round-2 item 4: auto-zoom the pane hosting a
    /// modal). Explicit `on`/`off` is used rather than `toggle` so this is
    /// idempotent regardless of the pane's current zoom state.
    fn zoom_pane(&mut self, pane_id: &str, on: bool) -> Result<()>;
    /// `layout.export {tab_id}` — the tab's split tree (`layout.root`): split
    /// nodes carry `direction`/`ratio`/`first`/`second`, leaves `pane_id`.
    fn layout_export(&mut self, tab_id: &str) -> Result<Value>;
    /// `layout.set_split_ratio {tab_id, path, ratio}` — set one split's
    /// first-child share exactly. `path` walks from the root: `false` =
    /// first child, `true` = second (verified live, herdr 0.9.3). Unlike
    /// `pane swap`, it doesn't activate the tab.
    fn set_split_ratio(&mut self, tab_id: &str, path: &[bool], ratio: f64) -> Result<()>;
    /// `pane.list {}` — every pane in every workspace (there is no tab
    /// filter; callers filter on `tab_id`).
    fn list_panes(&mut self) -> Result<Vec<PaneEntry>>;
    /// `server.stop {}` — shut herdr down (auto-close's last-pane quit).
    fn stop_server(&mut self) -> Result<()>;

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
        UnixSocketClient {
            socket_path: socket_path_from_env(),
        }
    }
}

/// One NDJSON request/response round-trip on a fresh connection.
fn request(path: &Path, method: &str, params: serde_json::Value) -> anyhow::Result<Value> {
    let mut stream = UnixStream::connect(path)
        .with_context(|| format!("herdr socket unavailable at {}", path.display()))?;
    // Runs on the TUI main thread — never let a stalled herdr hang forever.
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .context("failed to set socket read timeout")?;
    stream
        .set_write_timeout(Some(REQUEST_TIMEOUT))
        .context("failed to set socket write timeout")?;
    let id = next_request_id();
    let req = serde_json::json!({"id": id, "method": method, "params": params});
    stream.write_all(format!("{req}\n").as_bytes())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let resp: Value = serde_json::from_str(&line).context("bad NDJSON from herdr")?;
    if resp.get("id").and_then(|v| v.as_str()) != Some(id.as_str()) {
        anyhow::bail!("response id mismatch");
    }
    if let Some(err) = resp.get("error") {
        anyhow::bail!(
            "herdr: {}",
            err.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
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
        request(
            &self.socket_path,
            "agent.focus",
            serde_json::json!({"target": target}),
        )?;
        Ok(())
    }
    fn focus_pane(&mut self, pane_id: &str) -> Result<()> {
        request(
            &self.socket_path,
            "pane.focus",
            serde_json::json!({"pane_id": pane_id}),
        )?;
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
        let tab_id =
            find_string_field(&result, "tab_id").context("no tab_id in tab.create response")?;
        let panes = request(&self.socket_path, "pane.list", serde_json::json!({}))?;
        result_entries(&panes)
            .into_iter()
            .find(|e| e.get("tab_id").and_then(|t| t.as_str()) == Some(tab_id.as_str()))
            .and_then(|e| Some(e.get("pane_id")?.as_str()?.to_string()))
            .context("created tab has no pane")
    }
    fn agent_start(
        &mut self,
        name: &str,
        kind: &str,
        pane_id: &str,
        args: &[String],
    ) -> Result<()> {
        request(
            &self.socket_path,
            "agent.start",
            serde_json::json!({"name": name, "kind": kind, "pane_id": pane_id, "args": args}),
        )?;
        Ok(())
    }
    fn zoom_pane(&mut self, pane_id: &str, on: bool) -> Result<()> {
        request(
            &self.socket_path,
            "pane.zoom",
            serde_json::json!({"pane_id": pane_id, "mode": if on { "on" } else { "off" }}),
        )?;
        Ok(())
    }
    fn layout_export(&mut self, tab_id: &str) -> Result<Value> {
        let result = request(
            &self.socket_path,
            "layout.export",
            serde_json::json!({"tab_id": tab_id}),
        )?;
        result
            .get("layout")
            .and_then(|l| l.get("root"))
            .cloned()
            .context("no layout.root in layout.export response")
    }
    fn set_split_ratio(&mut self, tab_id: &str, path: &[bool], ratio: f64) -> Result<()> {
        request(
            &self.socket_path,
            "layout.set_split_ratio",
            serde_json::json!({"tab_id": tab_id, "path": path, "ratio": ratio}),
        )?;
        Ok(())
    }
    fn list_panes(&mut self) -> Result<Vec<PaneEntry>> {
        Ok(parse_pane_list(&request(
            &self.socket_path,
            "pane.list",
            serde_json::json!({}),
        )?))
    }
    fn stop_server(&mut self) -> Result<()> {
        request(&self.socket_path, "server.stop", serde_json::json!({}))?;
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

/// Read timeout on the event-stream connection. Statuses now arrive as
/// per-pane pushes; each timeout still triggers a full resync as a fallback
/// poll until the push path is proven (spec #22), and keeps doing its
/// original job: unwedging a stalled connection that never closes.
const SUBSCRIBE_READ_TIMEOUT: Duration = Duration::from_secs(3);

/// Blocks streaming events until the connection drops (or the app goes away).
/// Subscription types per the research doc's recommended wiring.
pub fn subscribe_loop(path: &Path, tx: &mpsc::Sender<crate::tui::AppEvent>) -> Result<()> {
    subscribe_loop_with_timeout(path, tx, SUBSCRIBE_READ_TIMEOUT)
}

fn send(tx: &mpsc::Sender<crate::tui::AppEvent>, ev: crate::tui::AppEvent) -> Result<()> {
    tx.send(ev).map_err(|_| anyhow::anyhow!("app gone"))
}

/// Push a full agent list to the app; returns the agent pane ids (sorted),
/// which decide the per-pane status subscriptions.
fn resync(path: &Path, tx: &mpsc::Sender<crate::tui::AppEvent>) -> Result<Vec<String>> {
    let agents = parse_agent_list(&request(path, "agent.list", serde_json::json!({}))?);
    let mut panes: Vec<String> = agents.iter().map(|a| a.pane_id.clone()).collect();
    panes.sort();
    send(tx, crate::tui::AppEvent::Agents(agents))?;
    Ok(panes)
}

/// The `events.subscribe` request. `pane.agent_status_changed` is PER-PANE in
/// herdr (0.9.x rejects the whole request if it lacks a `pane_id`), so it is
/// asked for once per known agent pane; everything else is global.
fn subscribe_request(agent_panes: &[String]) -> Value {
    let mut subs = vec![
        serde_json::json!({"type": "pane.created"}),
        serde_json::json!({"type": "pane.closed"}),
        serde_json::json!({"type": "pane.agent_detected"}),
        serde_json::json!({"type": "layout.updated"}),
    ];
    subs.extend(
        agent_panes
            .iter()
            .map(|p| serde_json::json!({"type": "pane.agent_status_changed", "pane_id": p})),
    );
    serde_json::json!({"id": "sub", "method": "events.subscribe",
                       "params": {"subscriptions": subs}})
}

/// A pushed frame, decoded. Only what feedr acts on.
#[derive(Debug, PartialEq)]
enum Pushed {
    AgentStatus {
        pane_id: String,
        status: AgentStatus,
    },
    Layout {
        tab_id: String,
        pane_ids: Vec<String>,
    },
    /// A pane came, went or gained an agent: the agent set may have changed.
    PaneLifecycle,
}

/// Decode one pushed frame. herdr wraps every push as
/// `{"event": <name>, "data": {...}}` — never a top-level `type`. Global
/// subscriptions name events with underscores (`pane_closed`, and repeat it
/// as `data.type`); per-pane ones use the dotted subscription name
/// (`pane.agent_status_changed`) with no `data.type`. Both are accepted.
/// The subscribe ack (`{"id", "result"}`) and unknown events give `None`.
fn parse_pushed(v: &Value) -> Option<Pushed> {
    let event = v.get("event")?.as_str()?.replace('.', "_");
    let data = v.get("data")?;
    match event.as_str() {
        "pane_agent_status_changed" => Some(Pushed::AgentStatus {
            pane_id: data.get("pane_id")?.as_str()?.to_string(),
            status: AgentStatus::parse(data.get("agent_status")?.as_str()?),
        }),
        "layout_updated" => {
            let layout = data.get("layout")?;
            Some(Pushed::Layout {
                tab_id: layout.get("tab_id")?.as_str()?.to_string(),
                pane_ids: layout
                    .get("panes")?
                    .as_array()?
                    .iter()
                    .filter_map(|p| p.get("pane_id")?.as_str().map(str::to_string))
                    .collect(),
            })
        }
        "pane_created" | "pane_closed" | "pane_exited" | "pane_agent_detected" => {
            Some(Pushed::PaneLifecycle)
        }
        _ => None,
    }
}

/// `true` for the two platform-dependent error kinds a blocking read can
/// surface once its `set_read_timeout` deadline elapses (`WouldBlock` on
/// some platforms, `TimedOut` on others — handle both).
fn is_read_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn subscribe_loop_with_timeout(
    path: &Path,
    tx: &mpsc::Sender<crate::tui::AppEvent>,
    read_timeout: Duration,
) -> Result<()> {
    let mut panes = resync(path, tx)?;
    // herdr drops a connection that sends a second `events.subscribe`, so
    // when the agent set changes the stream is replaced with a fresh one
    // carrying the new per-pane list — without reporting SocketDown.
    while let Some(changed) = stream_events(path, tx, read_timeout, &panes)? {
        panes = changed;
    }
    Ok(())
}

/// Stream one subscription. `Ok(None)` at EOF (a genuine disconnect, so the
/// caller's backoff loop reconnects); `Ok(Some(panes))` when a resync shows
/// the agent panes changed and the subscription must be rebuilt.
fn stream_events(
    path: &Path,
    tx: &mpsc::Sender<crate::tui::AppEvent>,
    read_timeout: Duration,
    panes: &[String],
) -> Result<Option<Vec<String>>> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(read_timeout))?;
    stream.write_all(format!("{}\n", subscribe_request(panes)).as_bytes())?;
    // Whatever was pushed before this stream (a replaced stream's unread
    // frames, a reconnect gap) is gone; let the app re-check what it missed.
    send(tx, crate::tui::AppEvent::Subscribed)?;
    let mut reader = BufReader::new(stream);
    let resync_and_check = || -> Result<Option<Vec<String>>> {
        let now = resync(path, tx)?;
        Ok((now != panes).then_some(now))
    };
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Ok(None),
            Ok(_) => {
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                // The subscribe ack and unknown events parse to None — skip.
                match parse_pushed(&v) {
                    Some(Pushed::AgentStatus { pane_id, status }) => {
                        send(tx, crate::tui::AppEvent::AgentStatus { pane_id, status })?
                    }
                    Some(Pushed::Layout { tab_id, pane_ids }) => {
                        send(tx, crate::tui::AppEvent::Layout { tab_id, pane_ids })?
                    }
                    Some(Pushed::PaneLifecycle) => {
                        if let Some(changed) = resync_and_check()? {
                            return Ok(Some(changed));
                        }
                    }
                    None => {}
                }
            }
            // Read timeout: the fallback poll (and the cure for a
            // stalled-but-open connection). Resync, then keep reading on
            // this SAME stream unless the agent set changed.
            Err(e) if is_read_timeout(&e) => {
                if let Some(changed) = resync_and_check()? {
                    return Ok(Some(changed));
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
#[derive(Default, Clone)]
pub struct FakeHerdr {
    pub fail: bool,
    pub agents: Vec<AgentInfo>,
    /// Shared so tests can keep a clone and inspect calls after boxing.
    pub log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    /// What `layout_export` returns; `None` makes it fail.
    pub layout: Option<Value>,
    /// What `list_panes` returns; `None` makes it fail. Shared so a test
    /// can change herdr's panes after the fake is boxed into the app.
    pub panes: std::rc::Rc<std::cell::RefCell<Option<Vec<PaneEntry>>>>,
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
    fn agent_start(
        &mut self,
        name: &str,
        kind: &str,
        pane_id: &str,
        args: &[String],
    ) -> Result<()> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        self.log.borrow_mut().push(format!(
            "agent_start {name} {kind} {pane_id} {}",
            args.join(" ")
        ));
        Ok(())
    }
    fn zoom_pane(&mut self, pane_id: &str, on: bool) -> Result<()> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        self.log.borrow_mut().push(format!(
            "zoom_pane {pane_id} {}",
            if on { "on" } else { "off" }
        ));
        Ok(())
    }
    fn layout_export(&mut self, _tab_id: &str) -> Result<Value> {
        self.layout.clone().context("no layout in fake")
    }
    fn set_split_ratio(&mut self, tab_id: &str, path: &[bool], ratio: f64) -> Result<()> {
        let path: Vec<&str> = path.iter().map(|b| if *b { "2" } else { "1" }).collect();
        self.log.borrow_mut().push(format!(
            "set_split_ratio {tab_id} [{}] {ratio:.4}",
            path.join(",")
        ));
        Ok(())
    }
    fn list_panes(&mut self) -> Result<Vec<PaneEntry>> {
        self.log.borrow_mut().push("list_panes".into());
        self.panes
            .borrow()
            .clone()
            .context("pane.list failed in fake")
    }
    fn stop_server(&mut self) -> Result<()> {
        self.log.borrow_mut().push("stop_server".into());
        Ok(())
    }
    // open_resume_tab: provided method — the fake exercises the real
    // composition, so tests see the create_tab + agent_start sequence.
}

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

    use std::os::unix::net::UnixListener;

    /// Sequential fake herdr: for each accepted connection, read one request
    /// line (recorded for assertions), write the scripted response lines, close.
    /// Any `{id}` placeholder in a script line is replaced with the id the
    /// client actually sent, so scripts don't need to know the process-wide
    /// request counter's current value.
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
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                let sent_id = serde_json::from_str::<Value>(line.trim())
                    .ok()
                    .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
                    .unwrap_or_default();
                seen2.lock().unwrap().push(line.trim().to_string());
                let mut w = stream;
                for l in script {
                    let l = l.replace("{id}", &sent_id);
                    let _ = writeln!(w, "{l}");
                }
            }
        });
        (dir, path, seen)
    }

    #[test]
    fn live_client_lists_agents_and_surfaces_errors() {
        let ok = serde_json::json!({"id": "{id}", "result": {"agents": [
            {"pane_id": "w1:p7", "agent": "claude", "agent_status": "blocked",
             "agent_session": {"value": "abc"}}]}})
        .to_string();
        let err =
            r#"{"id":"{id}","error":{"code":"not_found","message":"pane not found"}}"#.to_string();
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
        let mut c = UnixSocketClient {
            socket_path: dir.path().join("nope.sock"),
        };
        let e = c.list_agents().unwrap_err();
        assert!(
            e.to_string().contains("herdr socket unavailable"),
            "got: {e}"
        );
    }

    #[test]
    fn live_resume_flow_creates_tab_then_starts_agent() {
        let tab = r#"{"id":"{id}","result":{"tab_id":"w1:t9","pane_id":"w1:p9"}}"#.to_string();
        let ok = r#"{"id":"{id}","result":{}}"#.to_string();
        let (_dir, path, seen) = fake_server(vec![vec![tab], vec![ok]]);
        let mut c = UnixSocketClient { socket_path: path };
        c.open_resume_tab("claude", "abc-123").unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "wire: {seen:?}");
        assert!(
            seen[0].contains("\"method\":\"tab.create\""),
            "got: {}",
            seen[0]
        );
        assert!(seen[0].contains("\"label\":\"resume claude\""));
        assert!(
            seen[1].contains("\"method\":\"agent.start\""),
            "got: {}",
            seen[1]
        );
        assert!(seen[1].contains("\"kind\":\"claude\""));
        assert!(seen[1].contains("\"pane_id\":\"w1:p9\""));
        assert!(seen[1].contains("--resume"));
    }

    /// Round-2 item 4: `pane.zoom` verified live against herdr 0.9.0
    /// (`herdr pane zoom --help` and `herdr api schema --json`) —
    /// `PaneZoomParams { pane_id, mode: "toggle"|"on"|"off" }`. The sidebar
    /// always sends an explicit `mode` ("on"/"off") rather than "toggle" so
    /// zooming is idempotent regardless of the pane's current zoom state.
    #[test]
    fn live_client_zooms_pane_on_and_off() {
        let ok = r#"{"id":"{id}","result":{"changed":true,"zoom_changed":true,"focus_changed":false,"pane_id":"w1:p3","focused_pane_id":"w1:p3","zoomed":true}}"#.to_string();
        let (_dir, path, seen) = fake_server(vec![vec![ok.clone()], vec![ok]]);
        let mut c = UnixSocketClient { socket_path: path };
        c.zoom_pane("w1:p3", true).unwrap();
        c.zoom_pane("w1:p3", false).unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "wire: {seen:?}");
        assert!(
            seen[0].contains("\"method\":\"pane.zoom\""),
            "got: {}",
            seen[0]
        );
        assert!(seen[0].contains("\"pane_id\":\"w1:p3\""));
        assert!(seen[0].contains("\"mode\":\"on\""));
        assert!(seen[1].contains("\"mode\":\"off\""));
    }

    #[test]
    fn create_tab_falls_back_to_pane_list_for_pane_id() {
        // A tab.create response that names only the tab: resolve the pane
        // via pane.list filtered to the new tab_id.
        let tab = r#"{"id":"{id}","result":{"tab_id":"w1:t9"}}"#.to_string();
        // Kept on one line deliberately: the client reads one NDJSON line per
        // response (`request`'s `read_line`), so a literal newline embedded
        // in this fixture (as a pretty-printed multi-line raw string would
        // have) would truncate the parse mid-array.
        let panes = r#"{"id":"{id}","result":{"panes":[{"pane_id":"w1:p2","tab_id":"w1:t1"},{"pane_id":"w1:p9","tab_id":"w1:t9"}]}}"#.to_string();
        let (_dir, path, _seen) = fake_server(vec![vec![tab], vec![panes]]);
        let mut c = UnixSocketClient { socket_path: path };
        assert_eq!(c.create_tab("resume claude").unwrap(), "w1:p9");
    }

    /// Regression: herdr 0.9.0 requires a `pane_id` on a
    /// `pane.agent_status_changed` subscription — it is per-pane, not global.
    /// Asking for it globally made herdr reject the WHOLE subscribe request
    /// (`invalid_request: missing field pane_id`), so the stream EOF'd, the
    /// thread reported SocketDown, and every agent glyph in the sidebar was
    /// cleared every 5 seconds. Verified live against the socket, which the
    /// injected-statuses view tests could never catch.
    #[test]
    fn subscribe_never_asks_globally_for_a_per_pane_subscription() {
        let (_dir, path, seen) = fake_server(vec![
            vec![serde_json::json!({"id": "{id}", "result": {"agents": []}}).to_string()],
            vec![r#"{"id":"sub","result":{}}"#.to_string()],
        ]);
        let (tx, _rx) = std::sync::mpsc::channel();

        subscribe_loop(&path, &tx).unwrap();

        let sub = seen
            .lock()
            .unwrap()
            .iter()
            .find(|l| l.contains("events.subscribe"))
            .cloned()
            .expect("a subscribe request should have been sent");
        assert!(
            !sub.contains("agent_status_changed"),
            "subscribing globally to a per-pane event type gets the whole \
             request rejected: {sub}"
        );
        for kind in ["pane.created", "pane.closed", "pane.agent_detected"] {
            assert!(sub.contains(kind), "{kind} missing from {sub}");
        }
    }

    #[test]
    fn subscribe_loop_resyncs_on_events() {
        let list = serde_json::json!({"id": "{id}", "result": {"agents": [
            {"pane_id": "w1:p7", "agent": "claude", "agent_status": "working",
             "agent_session": {"value": "abc"}}]}})
        .to_string();
        let ack = r#"{"id":"sub","result":{}}"#.to_string();
        // herdr's real global-event frame shape (captured live, 0.9.3). The
        // old top-level-`type` frame here was why the broken dispatch passed.
        let event = r#"{"event":"pane_closed","data":{"type":"pane_closed","pane_id":"w1:p9","workspace_id":"w1"}}"#
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

    // ---- Live push path, against the shared fake socket in herdr's real
    // wire shapes (captured live from herdr 0.9.3). ----

    use crate::tui::fake_socket::FakeHerdrSocket;
    use crate::tui::AppEvent;

    fn agent_entry(pane: &str, status: &str, session: &str) -> Value {
        json!({"pane_id": pane, "agent": "claude", "agent_status": status,
               "agent_session": {"value": session}})
    }

    /// Start `subscribe_loop` against `fake` on a background thread, with
    /// the read-timeout poll pushed far out so only pushes can be observed.
    fn run_subscriber(fake: &FakeHerdrSocket) -> mpsc::Receiver<AppEvent> {
        let (tx, rx) = mpsc::channel();
        let path = fake.path().to_path_buf();
        std::thread::spawn(move || {
            let _ = subscribe_loop_with_timeout(&path, &tx, Duration::from_secs(60));
        });
        rx
    }

    /// Receive until `pred` matches, or panic after 3s.
    fn recv_until(rx: &mpsc::Receiver<AppEvent>, pred: impl Fn(&AppEvent) -> bool) -> AppEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let left = deadline
                .checked_duration_since(std::time::Instant::now())
                .expect("timed out waiting for the expected app event");
            let ev = rx
                .recv_timeout(left)
                .expect("timed out waiting for the expected app event");
            if pred(&ev) {
                return ev;
            }
        }
    }

    #[test]
    fn pushed_agent_status_in_herdrs_real_shape_reaches_the_app() {
        let fake = FakeHerdrSocket::start();
        fake.reply(
            "agent.list",
            json!({"agents": [agent_entry("w1:p7", "working", "abc")]}),
        );
        let rx = run_subscriber(&fake);
        fake.wait_for_subscriptions(1);

        // Per-pane subscription frames: dotted event name, no `data.type`.
        fake.push(json!({"event": "pane.agent_status_changed", "data": {
            "agent": "claude", "agent_status": "done", "pane_id": "w1:p7", "workspace_id": "w1"}}));

        let ev = recv_until(&rx, |e| matches!(e, AppEvent::AgentStatus { .. }));
        assert_eq!(
            ev,
            AppEvent::AgentStatus {
                pane_id: "w1:p7".into(),
                status: AgentStatus::Done
            }
        );
    }

    /// The `pane.agent_status_changed` entries of each subscribe request.
    fn status_subs(fake: &FakeHerdrSocket) -> Vec<Vec<Value>> {
        fake.requests_for("events.subscribe")
            .iter()
            .map(|r| {
                r["params"]["subscriptions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| s["type"] == "pane.agent_status_changed")
                    .cloned()
                    .collect()
            })
            .collect()
    }

    #[test]
    fn agent_status_is_subscribed_per_known_agent_pane() {
        let fake = FakeHerdrSocket::start();
        fake.reply(
            "agent.list",
            json!({"agents": [agent_entry("w1:p7", "idle", "a"), agent_entry("w1:p8", "idle", "b")]}),
        );
        let _rx = run_subscriber(&fake);
        fake.wait_for_subscriptions(1);

        assert_eq!(
            status_subs(&fake),
            vec![vec![
                json!({"type": "pane.agent_status_changed", "pane_id": "w1:p7"}),
                json!({"type": "pane.agent_status_changed", "pane_id": "w1:p8"}),
            ]]
        );
    }

    #[test]
    fn a_newly_detected_agent_gets_its_own_status_subscription() {
        let fake = FakeHerdrSocket::start();
        fake.reply(
            "agent.list",
            json!({"agents": [agent_entry("w1:p7", "idle", "a")]}),
        );
        let rx = run_subscriber(&fake);
        fake.wait_for_subscriptions(1);

        fake.reply(
            "agent.list",
            json!({"agents": [agent_entry("w1:p7", "idle", "a"), agent_entry("w1:p9", "idle", "c")]}),
        );
        fake.push(json!({"event": "pane_agent_detected", "data": {
            "type": "pane_agent_detected", "pane_id": "w1:p9", "workspace_id": "w1", "agent": "claude"}}));
        fake.wait_for_subscriptions(2);

        let latest = status_subs(&fake).pop().unwrap();
        assert!(
            latest.contains(&json!({"type": "pane.agent_status_changed", "pane_id": "w1:p9"})),
            "new agent pane not subscribed: {latest:?}"
        );

        // The resubscribed stream is live: a push for the new pane lands.
        fake.push(json!({"event": "pane.agent_status_changed", "data": {
            "agent": "claude", "agent_status": "blocked", "pane_id": "w1:p9", "workspace_id": "w1"}}));
        let ev = recv_until(&rx, |e| matches!(e, AppEvent::AgentStatus { .. }));
        assert_eq!(
            ev,
            AppEvent::AgentStatus {
                pane_id: "w1:p9".into(),
                status: AgentStatus::Blocked
            }
        );
    }

    #[test]
    fn every_new_subscription_tells_the_app_pushes_may_have_been_lost() {
        let fake = FakeHerdrSocket::start();
        fake.reply(
            "agent.list",
            json!({"agents": [agent_entry("w1:p7", "idle", "a")]}),
        );
        let rx = run_subscriber(&fake);
        fake.wait_for_subscriptions(1);
        recv_until(&rx, |e| *e == AppEvent::Subscribed);

        // The agent's pane exits: herdr pushes the agent change and the
        // layout together, and the replaced stream drops the layout unread.
        fake.reply("agent.list", json!({"agents": []}));
        fake.push(json!({"event": "pane_agent_detected", "data": {
            "type": "pane_agent_detected", "pane_id": "w1:p7", "workspace_id": "w1"}}));
        fake.wait_for_subscriptions(2);
        recv_until(&rx, |e| *e == AppEvent::Subscribed);
    }

    #[test]
    fn layout_updates_are_subscribed_and_delivered_with_tab_and_panes() {
        let fake = FakeHerdrSocket::start();
        fake.reply("agent.list", json!({"agents": []}));
        let rx = run_subscriber(&fake);
        fake.wait_for_subscriptions(1);

        let sub = &fake.requests_for("events.subscribe")[0];
        assert!(
            sub["params"]["subscriptions"]
                .as_array()
                .unwrap()
                .contains(&json!({"type": "layout.updated"})),
            "layout.updated not subscribed: {sub}"
        );

        // Trimmed from a live herdr 0.9.3 capture.
        fake.push(json!({"event": "layout_updated", "data": {"type": "layout_updated", "layout": {
            "workspace_id": "w4", "tab_id": "w4:tB", "focused_pane_id": "w4:p1J", "zoomed": false,
            "panes": [
                {"pane_id": "w4:p1J", "focused": true, "rect": {"x": 0, "y": 0, "width": 40, "height": 20}},
                {"pane_id": "w4:p1K", "focused": false, "rect": {"x": 40, "y": 0, "width": 40, "height": 20}}
            ],
            "splits": []}}}));

        let ev = recv_until(&rx, |e| matches!(e, AppEvent::Layout { .. }));
        assert_eq!(
            ev,
            AppEvent::Layout {
                tab_id: "w4:tB".into(),
                pane_ids: vec!["w4:p1J".into(), "w4:p1K".into()],
            }
        );
    }

    #[test]
    fn request_times_out_on_stalled_connection() {
        // Accepts the connection but never reads or responds — simulates a
        // stalled herdr. `request()` (via focus_agent) must not hang the
        // caller (the TUI main thread) forever.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                std::thread::sleep(std::time::Duration::from_secs(30));
                drop(stream);
            }
        });

        let mut c = UnixSocketClient { socket_path: path };
        let start = std::time::Instant::now();
        let err = c.focus_agent("w1:p1").unwrap_err();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "request took {elapsed:?}, expected it to time out well under 5s"
        );
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn mismatched_response_id_is_rejected() {
        // The fake server deliberately ignores the sent id and replies with
        // a different one — `request()` must reject it rather than trust
        // whatever comes back first.
        let bad = r#"{"id":"not-the-request-id","result":{}}"#.to_string();
        let (_dir, path, _seen) = fake_server(vec![vec![bad]]);
        let mut c = UnixSocketClient { socket_path: path };
        let e = c.focus_agent("w1:p1").unwrap_err();
        assert!(e.to_string().contains("response id mismatch"), "got: {e}");
    }

    #[test]
    fn classifies_would_block_and_timed_out_as_read_timeouts() {
        use std::io::{Error, ErrorKind};
        assert!(is_read_timeout(&Error::from(ErrorKind::WouldBlock)));
        assert!(is_read_timeout(&Error::from(ErrorKind::TimedOut)));
        assert!(!is_read_timeout(&Error::from(ErrorKind::ConnectionReset)));
        assert!(!is_read_timeout(&Error::from(ErrorKind::UnexpectedEof)));
    }

    #[test]
    fn subscribe_loop_self_heals_on_stalled_read_then_exits_on_eof() {
        // A subscribe connection that goes silent (open but stalled) must
        // not wedge the loop: the read times out, we resync, and keep
        // reading on the SAME connection rather than erroring out.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap();

        fn respond_agent_list(stream: &mut UnixStream) {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let id = serde_json::from_str::<Value>(line.trim())
                .ok()
                .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_string))
                .unwrap_or_default();
            let resp = serde_json::json!({"id": id, "result": {"agents": [
                {"pane_id": "w1:p7", "agent": "claude", "agent_status": "working",
                 "agent_session": {"value": "abc"}}]}})
            .to_string();
            writeln!(stream, "{resp}").unwrap();
        }

        std::thread::spawn(move || {
            // Initial resync.
            let (mut a, _) = listener.accept().unwrap();
            respond_agent_list(&mut a);
            drop(a);

            // Subscribe connection: read the request, then go silent —
            // holding it open (not dropped yet) simulates a stall.
            let (b, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(b.try_clone().unwrap());
            let mut line = String::new();
            let _ = reader.read_line(&mut line);

            // The client's read times out and it resyncs on a fresh
            // connection — respond to that here.
            let (mut c, _) = listener.accept().unwrap();
            respond_agent_list(&mut c);
            drop(c);

            // Now end the stalled connection: the client's next read on it
            // sees EOF and returns cleanly (existing backoff path).
            drop(b);
        });

        let (tx, rx) = std::sync::mpsc::channel();
        let start = std::time::Instant::now();
        subscribe_loop_with_timeout(&path, &tx, std::time::Duration::from_millis(150)).unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "took {elapsed:?}"
        );

        let mut agent_batches = 0;
        while let Ok(ev) = rx.try_recv() {
            if matches!(ev, crate::tui::AppEvent::Agents(_)) {
                agent_batches += 1;
            }
        }
        assert_eq!(
            agent_batches, 2,
            "expected the initial resync plus one stall self-heal resync"
        );
    }

    #[test]
    fn list_panes_reads_herdrs_pane_list_and_resolves_our_live_tab() {
        let fake = FakeHerdrSocket::start();
        // PaneInfo shape per `herdr api schema --json` (0.9.3), trimmed.
        fake.reply(
            "pane.list",
            json!({"type": "pane_list", "panes": [
                {"pane_id": "w1:p1", "tab_id": "w1:t3", "workspace_id": "w1", "focused": false,
                 "terminal_id": "t1", "agent_status": "unknown", "revision": 1,
                 "terminal_title_stripped": "feedr-sidebar"},
                {"pane_id": "w1:p2", "tab_id": "w1:t3", "workspace_id": "w1", "focused": true,
                 "terminal_id": "t2", "agent_status": "unknown", "revision": 1,
                 "terminal_title": "zsh"}]}),
        );
        let mut client = UnixSocketClient {
            socket_path: fake.path().to_path_buf(),
        };
        let panes = client.list_panes().unwrap();
        assert_eq!(
            panes,
            vec![
                PaneEntry {
                    pane_id: "w1:p1".into(),
                    tab_id: "w1:t3".into(),
                    title: Some("feedr-sidebar".into()),
                },
                PaneEntry {
                    pane_id: "w1:p2".into(),
                    tab_id: "w1:t3".into(),
                    title: Some("zsh".into()),
                },
            ]
        );
        assert!(panes[0].is_feedr() && !panes[1].is_feedr());
        assert_eq!(own_tab(&panes, "w1:p1"), Some("w1:t3"));
        assert_eq!(own_tab(&panes, "w2:p1"), None);
        assert_eq!(fake.requests_for("pane.list")[0]["params"], json!({}));
    }

    #[test]
    fn stop_server_sends_server_stop_and_surfaces_errors() {
        let fake = FakeHerdrSocket::start();
        let mut client = UnixSocketClient {
            socket_path: fake.path().to_path_buf(),
        };
        assert!(client.stop_server().is_err(), "unscripted: herdr error");
        fake.reply("server.stop", json!({"type": "ok"}));
        client.stop_server().unwrap();
        assert_eq!(fake.requests_for("server.stop").len(), 2);
    }
}
