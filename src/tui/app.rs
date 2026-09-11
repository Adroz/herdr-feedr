use crate::config::SidebarConfig;
use crate::feed::ops::{self, Authority, Zone};
use crate::feed::write::save_atomic;
use crate::feed::{AgentRef, Document, Item, Node, State};
use crate::tui::modal::{EditModal, Modal, ModalStep};
use crate::tui::socket::{AgentInfo, Herdr};
use anyhow::Result;
use crossterm::event::Event;
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
        ItemKey {
            title: it.title.clone(),
            state: it.state,
        }
    }
}

/// Relocate a keyed item in the active zones of the document.
/// Returns the index of the item if found and unambiguous (exactly one match).
/// A miss OR an ambiguous match (two active items sharing title+state) drops
/// the action — never guess which item the user meant.
pub fn relocate(doc: &Document, key: &ItemKey) -> Option<usize> {
    let matches: Vec<usize> = doc
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| match n {
            Node::Item(it)
                if ops::zone_of(doc, i) != Zone::Archive
                    && it.title == key.title
                    && it.state == key.state =>
            {
                Some(i)
            }
            _ => None,
        })
        .collect();

    match matches.len() {
        1 => Some(matches[0]),
        _ => None,
    }
}

/// One display row of the sidebar list.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Section(String),
    Item {
        key: ItemKey,
        agent: Option<AgentRef>,
    },
    AgentLine {
        key: ItemKey,
        agent: AgentRef,
    },
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
                rows.push(Row::Item {
                    key: key.clone(),
                    agent: it.agent.clone(),
                });
                if let Some(a) = &it.agent {
                    rows.push(Row::AgentLine {
                        key,
                        agent: a.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    rows.push(Row::Add);
    (rows, done_count)
}

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

/// Whether a `with_feed` closure actually mutated the document — controls
/// whether `with_feed` bothers to save. Every dropped-action path (relocate
/// miss, no-op sweep, etc.) must report `Unchanged` so a no-op click doesn't
/// rewrite the file (and self-trigger the file watcher for nothing).
#[derive(Debug)]
pub enum Outcome {
    Changed(Option<String>),
    Unchanged(Option<String>),
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
    pub modal: Modal,
    /// $EDITOR captured at startup; injectable in tests (avoids process-global
    /// env mutation racing parallel tests).
    pub editor_cmd: Option<String>,
    editor_request: Option<PathBuf>,
    /// Our own herdr pane id (env HERDR_PANE_ID), when running inside herdr.
    pub herdr_pane_id: Option<String>,
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
            modal: Modal::None,
            editor_cmd: std::env::var("EDITOR").ok().filter(|e| !e.is_empty()),
            editor_request: None,
            herdr_pane_id: std::env::var("HERDR_PANE_ID")
                .ok()
                .filter(|p| !p.is_empty()),
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

    /// Read fresh → apply one change → atomic save (only if it mutated) →
    /// reload. This is the ONLY path that writes the feed from the TUI
    /// (spec §6).
    ///
    /// Unlike `reload`, a missing file here is not automatically treated as
    /// an empty document — that distinction matters mid-action:
    ///
    /// - First run: the default feed path simply doesn't exist yet. If we
    ///   dropped every action on NotFound, a new user could never create
    ///   their first item. When the in-memory doc has no items at all
    ///   (nothing to lose), proceed with an empty `Document` so the action
    ///   goes through and `save_atomic` creates the file.
    /// - Mid-session deletion: the file existed and was removed between the
    ///   last render and this action (e.g. a click). If the in-memory doc
    ///   HAS items, saving an empty document back would turn a transient
    ///   deletion into permanent data loss — so that case still drops the
    ///   action.
    fn with_feed<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Document) -> Result<Outcome>,
    {
        let mut doc = match std::fs::read_to_string(&self.feed_path) {
            Ok(t) => crate::feed::parse::parse(&t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let has_items = self.doc.nodes.iter().any(|n| matches!(n, Node::Item(_)));
                if has_items {
                    self.status_msg = Some("feed file missing; action dropped".into());
                    return;
                }
                Document::default()
            }
            Err(e) => {
                self.status_msg = Some(format!("cannot read feed: {e}"));
                return;
            }
        };
        match f(&mut doc) {
            Ok(Outcome::Changed(msg)) => match save_atomic(&doc, &self.feed_path) {
                Ok(()) => self.status_msg = msg,
                Err(e) => self.status_msg = Some(format!("save failed: {e}")),
            },
            Ok(Outcome::Unchanged(msg)) => self.status_msg = msg,
            Err(e) => self.status_msg = Some(e.to_string()),
        }
        self.reload();
    }

    pub fn apply(&mut self, action: Action) {
        self.status_msg = None;
        match action {
            Action::Quit => self.should_quit = true,
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
            Action::ScrollUp => self.scroll = self.scroll.saturating_sub(1),
            Action::ScrollDown => {
                let max = self.rows.len().saturating_sub(1);
                self.scroll = self.scroll.saturating_add(1).min(max);
            }
            Action::Advance(key) => self.advance(key),
            Action::Sweep => {
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                self.with_feed(|doc| {
                    let n = ops::sweep(doc, &today);
                    if n == 0 {
                        Ok(Outcome::Unchanged(Some("swept 0 item(s)".into())))
                    } else {
                        Ok(Outcome::Changed(Some(format!("swept {n} item(s)"))))
                    }
                });
            }
            Action::AgentClick(key) => self.agent_click(key),
            Action::OpenEdit(key) => {
                if let Some(i) = relocate(&self.doc, &key) {
                    if let Node::Item(it) = &self.doc.nodes[i] {
                        self.set_modal(Modal::Edit(EditModal::edit(key, it)));
                    }
                }
            }
            Action::OpenCreate => self.set_modal(Modal::Edit(EditModal::create())),
            Action::OpenDoneView => self.set_modal(Modal::DoneView { scroll: 0 }),
            Action::OpenFileView => self.set_modal(Modal::FileView { scroll: 0 }),
            Action::OpenEditor => self.request_editor(),
        }
    }

    /// Set the active modal, zooming the sidebar's herdr pane to fill the
    /// whole tab the moment a modal actually opens (round-2 item 4).
    /// `handle_modal_event` is the mirror-image close path.
    fn set_modal(&mut self, modal: Modal) {
        self.modal = modal;
        self.zoom(true);
    }

    /// Best-effort `pane.zoom`: outside herdr (no pane id discovered yet)
    /// this is a no-op — the modal simply stays in-pane. A herdr call that
    /// fails (socket down, etc.) degrades silently to a status message at
    /// most; it never blocks opening or closing the modal.
    fn zoom(&mut self, on: bool) {
        if let Some(pane) = self.herdr_pane_id.clone() {
            if let Err(e) = self.herdr.zoom_pane(&pane, on) {
                self.status_msg = Some(format!("zoom: {e}"));
            }
        }
    }

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
        crate::feed::write::render(&Document {
            nodes: self.doc.nodes[start..end].to_vec(),
        })
    }

    /// The feed file verbatim (read-only view).
    pub fn file_text(&self) -> String {
        std::fs::read_to_string(&self.feed_path).unwrap_or_default()
    }

    pub fn modal_active(&self) -> bool {
        !matches!(self.modal, Modal::None)
    }

    /// Route an input event into the active modal (main-list input is
    /// bypassed while a modal is open). The pure transition logic lives in
    /// `modal::step`; this is just the glue for the two outcomes that need
    /// filesystem access (`Save`, `Delete`), which only `App` can provide
    /// (`with_feed` is private to this module). `size` is the terminal
    /// (width, height) — needed so `modal::step` can re-derive the edit
    /// modal's drawn geometry for mouse hit-testing, same as
    /// `input::translate` does for the main list.
    pub fn handle_modal_event(&mut self, ev: Event, size: (u16, u16)) {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let area = ratatui::layout::Rect::new(0, 0, size.0, size.1);
        self.modal = match crate::tui::modal::step(modal, ev, area) {
            ModalStep::Continue(m) => m,
            ModalStep::Save(m) => self.save_modal(m),
            ModalStep::Delete(key) => {
                self.delete_item(key);
                Modal::None
            }
            ModalStep::OpenEditor => {
                self.request_editor();
                Modal::None
            }
        };
        // Round-2 item 4: the modal fully closed this step (Save/Cancel/Esc/
        // viewer close/confirmed delete) — unzoom. A transition that stays
        // inside the modal lifecycle (e.g. ConfirmDelete <-> Edit) must not
        // toggle zoom again.
        if matches!(self.modal, Modal::None) {
            self.zoom(false);
        }
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
                        Ok(Outcome::Changed(None))
                    }
                    None => Ok(Outcome::Unchanged(Some(
                        "item changed on disk — edit dropped".into(),
                    ))),
                });
            }
            None => {
                // Sidebar-created items are always the human's; they land
                // at the end of the first human section (round-2 item 5 —
                // the section picker was dead UI: agents create their own
                // items via the CLI into `## Agent`, and can relocate items
                // later by editing the feed).
                self.with_feed(move |doc| {
                    ops::add(doc, &title, &body, Zone::Human);
                    Ok(Outcome::Changed(None))
                });
            }
        }
        Modal::None
    }

    fn delete_item(&mut self, key: ItemKey) {
        self.with_feed(move |doc| match relocate(doc, &key) {
            Some(i) => {
                ops::remove(doc, i);
                Ok(Outcome::Changed(Some("deleted".into())))
            }
            None => Ok(Outcome::Unchanged(Some(
                "item changed on disk — delete dropped".into(),
            ))),
        });
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
                Ok(Outcome::Changed(None))
            }
            None => Ok(Outcome::Unchanged(Some(
                "item changed on disk — click dropped".into(),
            ))),
        });
    }

    /// @agent sub-line click (spec §3): pane live → agent.focus; pane gone →
    /// resume tab; herdr absent → status-line hint, feed untouched.
    fn agent_click(&mut self, key: ItemKey) {
        let Some(i) = relocate(&self.doc, &key) else {
            self.status_msg = Some("item changed on disk — click dropped".into());
            return;
        };
        let Node::Item(it) = &self.doc.nodes[i] else {
            return;
        };
        let Some(agent) = it.agent.clone() else {
            return;
        };
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

    pub fn on_event(&mut self, ev: crate::tui::AppEvent) {
        match ev {
            crate::tui::AppEvent::FeedChanged => self.reload(),
            crate::tui::AppEvent::Agents(list) => {
                self.statuses = list
                    .into_iter()
                    .map(|a| (a.session_id.clone(), a))
                    .collect();
            }
            crate::tui::AppEvent::SocketDown => self.statuses.clear(),
        }
    }
}

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
        let key = ItemKey {
            title: "Migrate CI".into(),
            state: State::InProgress,
        };
        let i = relocate(&doc, &key).unwrap();
        assert!(matches!(&doc.nodes[i], Node::Item(it) if it.title == "Migrate CI"));
        // State mismatch (item advanced under us) → miss:
        let stale = ItemKey {
            title: "Migrate CI".into(),
            state: State::Open,
        };
        assert_eq!(relocate(&doc, &stale), None);
        // Archived items are never relocated:
        let archived = ItemKey {
            title: "Old thing".into(),
            state: State::Done,
        };
        assert_eq!(relocate(&doc, &archived), None);
    }

    #[test]
    fn relocate_returns_none_on_ambiguous_key() {
        let doc = parse("# Feed\n\n- [ ] Ship it\n\n## Later\n\n- [ ] Ship it\n");
        let key = ItemKey {
            title: "Ship it".into(),
            state: State::Open,
        };
        assert_eq!(relocate(&doc, &key), None);
    }

    use crate::config::{Side, SidebarConfig};
    use crate::tui::modal::EditFocus;
    use crate::tui::socket::{AgentInfo, AgentStatus, FakeHerdr};

    fn test_cfg() -> SidebarConfig {
        SidebarConfig {
            side: Side::Left,
            width: 0.18,
            auto_dock: false,
        }
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
            ("Alpha", State::Open, "- [~] Alpha"),     // [ ] → [~]
            ("Beta", State::InProgress, "- [x] Beta"), // [~] → [x]
            ("Gamma", State::Review, "- [x] Gamma"),   // [?] click = accept → [x]
            ("Delta", State::Done, "- [ ] Delta"),     // [x] → [ ]
        ] {
            app.apply(Action::Advance(ItemKey {
                title: title.into(),
                state: from,
            }));
            assert!(
                feed_text(&app).contains(expect),
                "want {expect} in:\n{}",
                feed_text(&app)
            );
        }
    }

    #[test]
    fn advance_never_produces_review_state() {
        // No transition targets [?] — that's the agent's signal (spec §3).
        let (mut app, _fake, _dir) = app_on_disk("- [ ] A\n");
        app.apply(Action::Advance(ItemKey {
            title: "A".into(),
            state: State::Open,
        }));
        app.apply(Action::Advance(ItemKey {
            title: "A".into(),
            state: State::InProgress,
        }));
        app.apply(Action::Advance(ItemKey {
            title: "A".into(),
            state: State::Done,
        }));
        assert_eq!(feed_text(&app), "- [ ] A\n"); // full cycle, never [?]
    }

    #[test]
    fn stale_key_drops_action_with_message() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] Alpha\n");
        // Simulate a concurrent agent claim between render and click:
        std::fs::write(&app.feed_path, "- [~] Alpha @agent(claude:abc)\n").unwrap();
        app.apply(Action::Advance(ItemKey {
            title: "Alpha".into(),
            state: State::Open,
        }));
        assert!(feed_text(&app).contains("- [~] Alpha @agent(claude:abc)")); // untouched
        assert!(app
            .status_msg
            .as_deref()
            .unwrap()
            .contains("changed on disk"));
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
        app.apply(Action::AgentClick(ItemKey {
            title: "Beta".into(),
            state: State::InProgress,
        }));
        assert_eq!(fake.log.borrow().as_slice(), ["focus_agent w1:p7"]);
    }

    #[test]
    fn agent_click_without_pane_resumes_session() {
        let (mut app, fake, _dir) = app_on_disk("- [~] Beta @agent(claude:abc)\n");
        app.apply(Action::AgentClick(ItemKey {
            title: "Beta".into(),
            state: State::InProgress,
        }));
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

    /// Task 7 review follow-up: a relocate miss on agent-click must report a
    /// status message, matching advance's policy, instead of silently
    /// dropping the click.
    #[test]
    fn agent_click_relocate_miss_sets_status_message() {
        let (mut app, _fake, _dir) = app_on_disk("- [~] Beta @agent(claude:abc)\n");
        // Simulate the row changing underneath a stale click, in memory —
        // agent_click reads self.doc directly, it doesn't go through with_feed:
        app.doc = parse("- [ ] Beta\n");
        app.apply(Action::AgentClick(ItemKey {
            title: "Beta".into(),
            state: State::InProgress,
        }));
        assert!(app
            .status_msg
            .as_deref()
            .unwrap()
            .contains("changed on disk"));
    }

    #[test]
    fn agent_click_degrades_to_status_message_when_herdr_absent() {
        let (mut app, _fake, _dir) = app_on_disk("- [~] Beta @agent(claude:abc)\n");
        app.herdr = Box::new(FakeHerdr {
            fail: true,
            ..FakeHerdr::default()
        });
        app.apply(Action::AgentClick(ItemKey {
            title: "Beta".into(),
            state: State::InProgress,
        }));
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

    /// Task 6 review follow-up: scroll math must saturate, never
    /// underflow/overflow-panic, even when hammered past either bound.
    #[test]
    fn scroll_saturates_without_overflow_at_bounds() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] A\n- [ ] B\n");
        for _ in 0..10 {
            app.apply(Action::ScrollDown);
        }
        assert_eq!(app.scroll, app.rows.len() - 1);
        for _ in 0..10 {
            app.apply(Action::ScrollUp);
        }
        assert_eq!(app.scroll, 0);
    }

    /// Spec review follow-up: a stale key (relocate miss) must drop the
    /// action without rewriting the file at all — not just "same bytes",
    /// but no save_atomic call. We prove that by backdating the file's
    /// mtime before the click and asserting it — and the content — are
    /// completely untouched afterward. A buggy always-save implementation
    /// would bump the mtime to "now" via the atomic rename even though the
    /// rendered bytes happen to round-trip identically.
    #[test]
    fn dropped_action_does_not_rewrite_file() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] Alpha\n");
        // Simulate a concurrent agent claim between render and click, same
        // as `stale_key_drops_action_with_message`:
        std::fs::write(&app.feed_path, "- [~] Alpha @agent(claude:abc)\n").unwrap();

        // Backdate the mtime so any rewrite (even a byte-identical one via
        // save_atomic's tempfile-rename) is detectable.
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&app.feed_path)
            .unwrap()
            .set_modified(past)
            .unwrap();

        let content_before = std::fs::read_to_string(&app.feed_path).unwrap();
        let mtime_before = std::fs::metadata(&app.feed_path)
            .unwrap()
            .modified()
            .unwrap();

        // Stale key: UI still thinks the item is [ ] Open, but disk now has
        // it as [~] claimed by an agent — relocate() misses.
        app.apply(Action::Advance(ItemKey {
            title: "Alpha".into(),
            state: State::Open,
        }));

        let content_after = std::fs::read_to_string(&app.feed_path).unwrap();
        let mtime_after = std::fs::metadata(&app.feed_path)
            .unwrap()
            .modified()
            .unwrap();

        assert_eq!(
            content_after, content_before,
            "file content must not change"
        );
        assert_eq!(
            mtime_after, mtime_before,
            "file must not be rewritten at all"
        );
    }

    /// Spec review follow-up: if the feed file is deleted between the
    /// snapshot render and the click (mid-action read), the dropped-action
    /// path must NOT treat NotFound as an empty document and save that
    /// empty document back — that would turn a transient deletion into
    /// permanent data loss. The action is simply dropped.
    ///
    /// Seed is deliberately non-empty ("- [ ] Alpha" loaded into
    /// `app.doc` via `app_on_disk`'s `reload()`) so this exercises the
    /// "file existed and vanished mid-session" branch of `with_feed`'s
    /// NotFound handling, not the first-run branch covered by
    /// `with_feed_proceeds_on_missing_file_when_in_memory_doc_has_no_items`
    /// below.
    #[test]
    fn action_on_deleted_feed_drops_and_does_not_create_empty_file() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] Alpha\n");
        std::fs::remove_file(&app.feed_path).unwrap();

        app.apply(Action::Advance(ItemKey {
            title: "Alpha".into(),
            state: State::Open,
        }));

        assert!(
            !app.feed_path.exists(),
            "a deleted feed must not be recreated (even empty) by a dropped action"
        );
        let msg = app.status_msg.as_deref().unwrap();
        assert!(
            msg.contains("missing") || msg.contains("dropped"),
            "expected a dropped-action status message, got: {msg}"
        );
    }

    /// User-feedback bug: on first run the default feed path doesn't exist
    /// yet, so `with_feed`'s NotFound handling used to drop every action —
    /// new users could never create their first item. When the in-memory
    /// doc has no items (fresh/empty session, nothing to lose), a
    /// mid-action NotFound must proceed with an empty Document instead of
    /// dropping, letting `save_atomic` create the file via its
    /// `create_dir_all`.
    #[test]
    fn with_feed_proceeds_on_missing_file_when_in_memory_doc_has_no_items() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.md"); // deliberately never created
        let mut app = App::new(path.clone(), test_cfg(), Box::new(FakeHerdr::default()));
        assert!(!path.exists());

        app.with_feed(|doc| {
            ops::add(doc, "First item", &[], Zone::Human);
            Ok(Outcome::Changed(None))
        });

        assert!(path.exists(), "first-run create must create the feed file");
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("First item"));
    }

    /// Same bug, exercised through the real create flow (`a` → type title →
    /// ^S) rather than calling `with_feed` directly, so the fix is proven at
    /// the level the user actually hit it.
    #[test]
    fn create_modal_save_creates_feed_file_on_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.md"); // deliberately never created
        let mut app = App::new(path.clone(), test_cfg(), Box::new(FakeHerdr::default()));
        assert!(!path.exists());

        app.apply(Action::OpenCreate);
        assert!(app.modal_active());
        // Round-2 item 5: no section picker — Title is focused immediately.
        type_str(&mut app, "First item");
        press_ctrl(&mut app, 's');

        assert!(!app.modal_active());
        assert!(
            path.exists(),
            "first-run create-via-modal must create the feed file"
        );
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("First item"));
    }

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    /// Arbitrary but realistic terminal size for driving modal events in
    /// tests that don't care about exact click geometry (those compute
    /// their own coordinates via `modal::edit_layout`).
    const TEST_SIZE: (u16, u16) = (80, 24);

    fn press(app: &mut App, code: KeyCode) {
        app.handle_modal_event(
            Event::Key(KeyEvent::new(code, KeyModifiers::NONE)),
            TEST_SIZE,
        );
    }

    fn press_ctrl(app: &mut App, c: char) {
        app.handle_modal_event(
            Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)),
            TEST_SIZE,
        );
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    /// Scope change (round-2 feedback item 5): the create modal's section
    /// picker is dead UI — sidebar-created items are always the human's and
    /// always land at the end of the first human section (agents create
    /// their own items via the CLI into `## Agent`, and can relocate items
    /// later by editing the feed). Title is focused first on create (no
    /// Section field to Tab through).
    #[test]
    fn create_modal_adds_to_first_human_section() {
        let (mut app, _fake, _dir) = app_on_disk("# Feed\n\n- [ ] A\n\n## Later\n\n- [ ] L1\n");
        app.apply(Action::OpenCreate);
        assert!(app.modal_active());
        let Modal::Edit(m) = &app.modal else {
            panic!("expected edit modal")
        };
        assert_eq!(m.focus, EditFocus::Title);
        type_str(&mut app, "New item");
        press(&mut app, KeyCode::Enter); // → Body
        type_str(&mut app, "ctx");
        press_ctrl(&mut app, 's');
        assert!(!app.modal_active());
        assert!(feed_text(&app).contains("- [ ] A\n- [ ] New item\n  ctx\n"));
        // Never landed in the named "## Later" section.
        assert!(!feed_text(&app).contains("- [ ] L1\n- [ ] New item"));
    }

    #[test]
    fn create_modal_empty_title_rejected() {
        let (mut app, _fake, _dir) = app_on_disk("# Feed\n\n- [ ] A\n");
        app.apply(Action::OpenCreate);
        press_ctrl(&mut app, 's'); // empty title
        assert!(app.modal_active());
        assert_eq!(app.status_msg.as_deref(), Some("title required"));
    }

    #[test]
    fn edit_modal_updates_title_and_body() {
        let (mut app, _fake, _dir) = app_on_disk("- [~] Old @agent(claude:abc)\n  old body\n");
        app.apply(Action::OpenEdit(ItemKey {
            title: "Old".into(),
            state: State::InProgress,
        }));
        let Modal::Edit(m) = &app.modal else {
            panic!("expected edit modal")
        };
        assert_eq!(m.title_text(), "Old");
        assert_eq!(m.body_lines(), vec!["old body"]);
        type_str(&mut app, "er"); // cursor starts in the title
        press_ctrl(&mut app, 's');
        let text = feed_text(&app);
        assert!(
            text.contains("Older") && text.contains("@agent(claude:abc)"),
            "got:\n{text}"
        );
    }

    #[test]
    fn delete_requires_confirm() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] Doomed\n  ctx\n- [ ] Keeper\n");
        app.apply(Action::OpenEdit(ItemKey {
            title: "Doomed".into(),
            state: State::Open,
        }));
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
        app.apply(Action::OpenEdit(ItemKey {
            title: "A".into(),
            state: State::Open,
        }));
        type_str(&mut app, "bc");
        press(&mut app, KeyCode::Esc);
        assert!(!app.modal_active());
        assert_eq!(feed_text(&app), "- [ ] A\n");
    }

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

    // --- Round-2 item 4: modal auto-zoom -----------------------------------
    //
    // When the sidebar knows its own herdr pane id, opening any modal
    // (edit/create/DoneView/FileView) zooms that pane to fill the whole
    // tab; closing it (Save/Cancel/Esc/viewer close) unzooms. Outside herdr
    // (no pane id) it's a no-op — the modal stays in-pane.

    #[test]
    fn create_modal_zooms_pane_on_open_and_off_on_cancel() {
        let (mut app, fake, _dir) = app_on_disk("- [ ] A\n");
        app.herdr_pane_id = Some("w1:p1".into());
        app.apply(Action::OpenCreate);
        assert_eq!(fake.log.borrow().as_slice(), ["zoom_pane w1:p1 on"]);
        press(&mut app, KeyCode::Esc);
        assert!(!app.modal_active());
        assert_eq!(
            fake.log.borrow().as_slice(),
            ["zoom_pane w1:p1 on", "zoom_pane w1:p1 off"]
        );
    }

    #[test]
    fn edit_modal_zooms_pane_on_open_and_off_on_save() {
        let (mut app, fake, _dir) = app_on_disk("- [ ] A\n");
        app.herdr_pane_id = Some("w1:p1".into());
        app.apply(Action::OpenEdit(ItemKey {
            title: "A".into(),
            state: State::Open,
        }));
        assert_eq!(fake.log.borrow().as_slice(), ["zoom_pane w1:p1 on"]);
        press_ctrl(&mut app, 's');
        assert!(!app.modal_active());
        assert_eq!(
            fake.log.borrow().as_slice(),
            ["zoom_pane w1:p1 on", "zoom_pane w1:p1 off"]
        );
    }

    #[test]
    fn viewer_modals_zoom_pane_on_open_and_off_on_close() {
        let (mut app, fake, _dir) = app_on_disk("- [ ] A\n");
        app.herdr_pane_id = Some("w1:p1".into());

        app.apply(Action::OpenDoneView);
        assert_eq!(fake.log.borrow().as_slice(), ["zoom_pane w1:p1 on"]);
        press(&mut app, KeyCode::Esc);
        assert_eq!(
            fake.log.borrow().as_slice(),
            ["zoom_pane w1:p1 on", "zoom_pane w1:p1 off"]
        );

        fake.log.borrow_mut().clear();
        app.apply(Action::OpenFileView);
        assert_eq!(fake.log.borrow().as_slice(), ["zoom_pane w1:p1 on"]);
        press(&mut app, KeyCode::Esc);
        assert_eq!(
            fake.log.borrow().as_slice(),
            ["zoom_pane w1:p1 on", "zoom_pane w1:p1 off"]
        );
    }

    /// The ConfirmDelete sub-state stays inside the same "modal open"
    /// lifecycle — entering/backing out of it must not toggle zoom again;
    /// only the eventual transition to `Modal::None` (confirmed delete)
    /// unzooms.
    #[test]
    fn confirm_delete_substate_does_not_toggle_zoom() {
        let (mut app, fake, _dir) = app_on_disk("- [ ] Doomed\n");
        app.herdr_pane_id = Some("w1:p1".into());
        app.apply(Action::OpenEdit(ItemKey {
            title: "Doomed".into(),
            state: State::Open,
        }));
        assert_eq!(fake.log.borrow().as_slice(), ["zoom_pane w1:p1 on"]);
        press_ctrl(&mut app, 'd'); // -> ConfirmDelete
        press(&mut app, KeyCode::Char('n')); // back out to Edit
        assert!(matches!(app.modal, Modal::Edit(_)));
        assert_eq!(
            fake.log.borrow().as_slice(),
            ["zoom_pane w1:p1 on"],
            "no extra zoom toggles while staying inside the modal"
        );
        press_ctrl(&mut app, 'd');
        press(&mut app, KeyCode::Char('y')); // confirm delete
        assert!(!app.modal_active());
        assert_eq!(
            fake.log.borrow().as_slice(),
            ["zoom_pane w1:p1 on", "zoom_pane w1:p1 off"]
        );
    }

    #[test]
    fn modal_zoom_is_noop_without_herdr_pane_id() {
        let (mut app, fake, _dir) = app_on_disk("- [ ] A\n");
        app.herdr_pane_id = None; // outside herdr / pane id not yet discovered
        app.apply(Action::OpenCreate);
        press(&mut app, KeyCode::Esc);
        assert!(
            fake.log.borrow().is_empty(),
            "no herdr calls when pane id absent"
        );
    }

    #[test]
    fn modal_zoom_failure_degrades_silently() {
        let (mut app, _fake, _dir) = app_on_disk("- [ ] A\n");
        app.herdr_pane_id = Some("w1:p1".into());
        app.herdr = Box::new(FakeHerdr {
            fail: true,
            ..FakeHerdr::default()
        });
        app.apply(Action::OpenCreate);
        assert!(app.modal_active(), "modal still opens even if zoom fails");
        press(&mut app, KeyCode::Esc);
        assert!(
            !app.modal_active(),
            "modal still closes even if unzoom fails"
        );
    }
}
