use crate::config::SidebarConfig;
use crate::feed::ops::{self, Authority, Zone};
use crate::feed::write::save_atomic;
use crate::feed::{AgentRef, Document, Item, Node, State};
use crate::tui::socket::{AgentInfo, Herdr};
use anyhow::Result;
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

/// Where a created item can go: the first human region, a named `##` human
/// section, or the reserved `## Agent` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionChoice {
    FirstHuman,
    Named(String),
    Agent,
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

/// Whether a `with_feed` closure actually mutated the document — controls
/// whether `with_feed` bothers to save. Every dropped-action path (relocate
/// miss, no-op sweep, etc.) must report `Unchanged` so a no-op click doesn't
/// rewrite the file (and self-trigger the file watcher for nothing).
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

    /// Read fresh → apply one change → atomic save (only if it mutated) →
    /// reload. This is the ONLY path that writes the feed from the TUI
    /// (spec §6).
    ///
    /// Unlike `reload`, a missing file here is NOT treated as an empty
    /// document: the file may have been deleted between the last render and
    /// this action (e.g. a click), and saving an empty document back would
    /// turn a transient deletion into permanent data loss. So a mid-action
    /// NotFound just drops the action.
    fn with_feed<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Document) -> Result<Outcome>,
    {
        let text = match std::fs::read_to_string(&self.feed_path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.status_msg = Some("feed file missing; action dropped".into());
                return;
            }
            Err(e) => {
                self.status_msg = Some(format!("cannot read feed: {e}"));
                return;
            }
        };
        let mut doc = crate::feed::parse::parse(&text);
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
            Action::ToggleCollapse => self.collapsed = !self.collapsed,
            Action::ScrollUp => self.scroll = self.scroll.saturating_sub(1),
            Action::ScrollDown => {
                let max = self.rows.len().saturating_sub(1);
                self.scroll = self.scroll.saturating_add(1).min(max);
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
                    if n == 0 {
                        Ok(Outcome::Unchanged(Some("swept 0 item(s)".into())))
                    } else {
                        Ok(Outcome::Changed(Some(format!("swept {n} item(s)"))))
                    }
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
}
