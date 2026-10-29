use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    InProgress,
    Review,
    Done,
}

impl State {
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            ' ' => Some(State::Open),
            '~' => Some(State::InProgress),
            '?' => Some(State::Review),
            'x' | 'X' => Some(State::Done),
            _ => None,
        }
    }
    pub fn to_char(self) -> char {
        match self {
            State::Open => ' ',
            State::InProgress => '~',
            State::Review => '?',
            State::Done => 'x',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRef {
    pub kind: String,
    pub id: String,
}

impl AgentRef {
    pub fn parse(s: &str) -> Option<Self> {
        let (kind, id) = s.split_once(':')?;
        if kind.is_empty() || id.is_empty() {
            return None;
        }
        Some(AgentRef {
            kind: kind.to_string(),
            id: id.to_string(),
        })
    }
}

impl fmt::Display for AgentRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub state: State,
    pub title: String,
    pub agent: Option<AgentRef>,
    /// Carries the trailing `@today` token: part of the human's Today.
    pub today: bool,
    /// Set on swept items: YYYY-MM-DD.
    pub done_date: Option<String>,
    /// Body lines without their two-space indent.
    /// Invariant: interior empty strings represent blank lines within a body
    /// (e.g. a blank line inside a fenced code block or between paragraphs);
    /// leading/trailing empty body lines are not allowed.
    pub body: Vec<String>,
    /// How the item was typed in the file, so hand edits survive a rewrite.
    pub style: ItemStyle,
}

/// Hand-typed formatting of one item that the canonical renderer would
/// otherwise normalise away. `Default` is canonical.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ItemStyle {
    /// The checkbox character as typed (e.g. `X`), when it isn't canonical.
    /// Forgotten by [`Item::set_state`], so any state change by feedr writes
    /// the canonical checkbox.
    pub mark: Option<char>,
    /// Body lines were indented with a tab rather than two spaces. Taken
    /// from the first body line and applied to the whole body on write.
    pub tab_body: bool,
}

impl Item {
    /// Change state. A real change drops the hand-typed checkbox so the
    /// canonical one is written; setting the same state keeps it. Completing
    /// leaves Today: every path to `[x]` comes through here, so the file
    /// keeps no trace of past focus.
    pub fn set_state(&mut self, state: State) {
        if state != self.state {
            self.state = state;
            self.style.mark = None;
            if state == State::Done {
                self.today = false;
            }
        }
    }

    /// The checkbox character to write: as typed, unless feedr changed the
    /// state since parsing.
    pub fn checkbox_char(&self) -> char {
        self.style.mark.unwrap_or(self.state.to_char())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// `# Title` (level 1) or `## Title` (level 2), text without the hashes.
    Heading {
        level: u8,
        text: String,
    },
    Item(Item),
    /// Any line the parser doesn't own — re-emitted verbatim.
    Raw(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    pub nodes: Vec<Node>,
    pub style: FileStyle,
}

/// File-wide formatting recorded at parse and reproduced on write.
/// `Default` is canonical: LF endings with a trailing newline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FileStyle {
    pub crlf: bool,
    pub no_trailing_newline: bool,
}

pub mod ops;
pub mod parse;
pub mod write;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_char_round_trip() {
        for (c, s) in [
            (' ', State::Open),
            ('~', State::InProgress),
            ('?', State::Review),
            ('x', State::Done),
        ] {
            assert_eq!(State::from_char(c), Some(s));
            assert_eq!(s.to_char(), c);
        }
        assert_eq!(State::from_char('z'), None);
    }

    #[test]
    fn agent_ref_parses_kind_and_id() {
        let a = AgentRef::parse("claude:0198f3ab-7c2e").unwrap();
        assert_eq!(a.kind, "claude");
        assert_eq!(a.id, "0198f3ab-7c2e");
        assert_eq!(a.to_string(), "claude:0198f3ab-7c2e");
        assert!(AgentRef::parse("no-colon").is_none());
        assert!(AgentRef::parse(":id").is_none());
        assert!(AgentRef::parse("kind:").is_none());
    }

    // Hand-edit preservation: feedr never silently changes formatting it
    // didn't need to touch. Exercised through parse -> op -> render.
    mod hand_edits {
        use crate::feed::ops::{self, Authority};
        use crate::feed::parse::parse;
        use crate::feed::write::render;
        use crate::feed::{Document, Node, State};

        fn item_at(doc: &Document, title: &str) -> usize {
            doc.nodes
                .iter()
                .position(|n| matches!(n, Node::Item(it) if it.title == title))
                .unwrap_or_else(|| panic!("no item titled {title:?}"))
        }

        #[test]
        fn capital_x_reads_as_done_and_survives_an_unrelated_edit() {
            let mut doc = parse("# Feed\n\n- [X] Shipped\n- [ ] Other\n");
            let shipped = item_at(&doc, "Shipped");
            assert!(matches!(&doc.nodes[shipped], Node::Item(it) if it.state == State::Done));
            let other = item_at(&doc, "Other");
            ops::edit(&mut doc, other, "Other, renamed", &[]);
            assert_eq!(
                render(&doc),
                "# Feed\n\n- [X] Shipped\n- [ ] Other, renamed\n"
            );
        }

        #[test]
        fn changing_a_capital_x_items_state_writes_the_canonical_checkbox() {
            let mut doc = parse("# Feed\n\n- [X] Shipped\n");
            let shipped = item_at(&doc, "Shipped");
            ops::set_state(&mut doc, shipped, State::Open, Authority::Human).unwrap();
            assert_eq!(render(&doc), "# Feed\n\n- [ ] Shipped\n");
            // Back to done: canonical, not the remembered capital.
            ops::set_state(&mut doc, shipped, State::Done, Authority::Human).unwrap();
            assert_eq!(render(&doc), "# Feed\n\n- [x] Shipped\n");
        }

        #[test]
        fn crlf_file_stays_crlf_through_an_edit() {
            let mut doc = parse("# Feed\r\n\r\n- [ ] A\r\n  body\r\n- [ ] B\r\n");
            let a = item_at(&doc, "A");
            assert!(matches!(&doc.nodes[a], Node::Item(it) if it.body == vec!["body"]));
            ops::add(&mut doc, "C", &[], ops::Zone::Human);
            assert_eq!(
                render(&doc),
                "# Feed\r\n\r\n- [ ] A\r\n  body\r\n- [ ] B\r\n- [ ] C\r\n"
            );
        }

        #[test]
        fn missing_trailing_newline_stays_missing_through_an_edit() {
            let mut doc = parse("# Feed\n\n- [ ] A\n- [ ] B");
            let a = item_at(&doc, "A");
            ops::edit(&mut doc, a, "A2", &[]);
            assert_eq!(render(&doc), "# Feed\n\n- [ ] A2\n- [ ] B");
            // Still missing when feedr appends the last line itself.
            ops::add(&mut doc, "C", &[], ops::Zone::Human);
            assert_eq!(render(&doc), "# Feed\n\n- [ ] A2\n- [ ] B\n- [ ] C");
        }

        #[test]
        fn tab_indented_body_parses_as_body_and_keeps_its_tabs() {
            let mut doc = parse("# Feed\n\n- [ ] A\n\tpara one\n\n\tpara two\n- [ ] B\n");
            let a = item_at(&doc, "A");
            assert!(
                matches!(&doc.nodes[a], Node::Item(it) if it.body == vec!["para one", "", "para two"]),
                "got {:?}",
                doc.nodes[a]
            );
            let b = item_at(&doc, "B");
            ops::edit(&mut doc, b, "B2", &[]);
            assert_eq!(
                render(&doc),
                "# Feed\n\n- [ ] A\n\tpara one\n\n\tpara two\n- [ ] B2\n"
            );
        }

        #[test]
        fn every_rule_together_survives_an_unrelated_edit() {
            let text = "# Feed\r\n\r\n- [X] Shipped\r\n- [ ] Other";
            let mut doc = parse(text);
            let other = item_at(&doc, "Other");
            ops::edit(&mut doc, other, "Renamed", &[]);
            assert_eq!(render(&doc), "# Feed\r\n\r\n- [X] Shipped\r\n- [ ] Renamed");
        }
    }
}
