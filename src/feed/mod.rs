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
            'x' => Some(State::Done),
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
    /// Set on swept items: YYYY-MM-DD.
    pub done_date: Option<String>,
    /// Body lines without their two-space indent.
    /// Invariant: interior empty strings represent blank lines within a body
    /// (e.g. a blank line inside a fenced code block or between paragraphs);
    /// leading/trailing empty body lines are not allowed.
    pub body: Vec<String>,
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
}
