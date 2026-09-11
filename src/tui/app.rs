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
}
