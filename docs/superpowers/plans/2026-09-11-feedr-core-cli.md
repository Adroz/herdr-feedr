# feedr Core + CLI Implementation Plan (Plan 1 of 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A working `feedr` binary: parses/writes the feed markdown file losslessly, applies all item operations atomically, and exposes them as CLI subcommands (`list/show/claim/add/review/done/sweep`).

**Architecture:** One Rust crate. The document model is a lossless sequence of nodes (headings, items, raw lines) so any human formatting survives a round-trip. All mutations go read-fresh → modify → atomic-write (temp file + rename). The CLI is a thin clap layer over `ops`; the future TUI (Plan 2) calls the same `ops` functions directly. Authority (spec §2) is a parameter: CLI defaults to Agent, `--as-human` overrides; the TUI will pass Human.

**Tech Stack:** Rust (edition 2021), clap (derive), anyhow, thiserror, chrono, tempfile. Dev: assert_cmd, predicates.

**Spec:** `docs/SPEC.md`. Format grammar implemented here: §1, §2, §2a. CLI: §4. Concurrency: §6.

---

## File structure

```
Cargo.toml
src/
  main.rs          — clap entry, subcommand dispatch only
  cli.rs           — clap types + command handlers (thin: resolve path, load, call ops, save, print)
  config.rs        — feed path resolution: --file > FEEDR_FEED env > config.toml feed_path > default
  feed/
    mod.rs         — pub use; Document/Node/Item/State/AgentRef types
    parse.rs       — text → Document (lossless)
    write.rs       — Document → text; atomic save
    ops.rs         — find/advance/claim/add/review/done/sweep; Authority enum
tests/
  cli.rs           — end-to-end assert_cmd tests against a temp feed file
```

Body lines are stored without their 2-space indent; the writer re-adds it. Everything the parser doesn't recognise becomes `Node::Raw` and is re-emitted verbatim.

---

### Task 1: Crate scaffold

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `.gitignore`

- [ ] **Step 1: Initialise the crate**

```bash
cd ~/development/herdr-feedr
cargo init --name feedr
```

- [ ] **Step 2: Write Cargo.toml**

```toml
[package]
name = "feedr"
version = "0.1.0"
edition = "2021"
description = "The feed rack for your herd — markdown to-do list shared with coding agents"
license = "MIT"

[dependencies]
anyhow = "1"
thiserror = "2"
clap = { version = "4", features = ["derive"] }
chrono = "0.4"
tempfile = "3"
toml = "0.8"
serde = { version = "1", features = ["derive"] }
dirs = "6"

[dev-dependencies]
assert_cmd = "2"
predicates = "3"
```

- [ ] **Step 3: Stub main.rs**

```rust
fn main() {
    println!("feedr");
}
```

- [ ] **Step 4: Verify it builds**

Run: `cargo build`
Expected: compiles clean.

- [ ] **Step 5: Append Rust entries to .gitignore and commit**

```bash
printf '/target\n' >> .gitignore
git add Cargo.toml Cargo.lock src/main.rs .gitignore
git commit -m "chore: scaffold feedr crate"
```

---

### Task 2: Document model + state chars

**Files:**
- Create: `src/feed/mod.rs`
- Modify: `src/main.rs` (add `mod feed;`)

- [ ] **Step 1: Write the failing test** (in `src/feed/mod.rs` bottom)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_char_round_trip() {
        for (c, s) in [(' ', State::Open), ('~', State::InProgress), ('?', State::Review), ('x', State::Done)] {
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
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test`
Expected: FAIL — `State`/`AgentRef` not defined.

- [ ] **Step 3: Implement the model**

```rust
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
        Some(AgentRef { kind: kind.to_string(), id: id.to_string() })
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
    pub body: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// `# Title` (level 1) or `## Title` (level 2), text without the hashes.
    Heading { level: u8, text: String },
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
```

(Create empty `src/feed/parse.rs`, `src/feed/write.rs`, `src/feed/ops.rs` so the module declarations compile. In `src/main.rs` add `mod feed;` above `fn main`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "feat: feed document model with four states and agent refs"
```

---

### Task 3: Parser (lossless)

**Files:**
- Create: `src/feed/parse.rs`

- [ ] **Step 1: Write the failing tests**

```rust
use super::*;

pub fn parse(text: &str) -> Document {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# Feed

- [ ] Fix auth redirect loop
  Repro: bounces forever.
  See #142.
- [~] Migrate CI @agent(claude:0198f3ab)
- [x] Bump Node to 22

## Later

- [ ] Evaluate pnpm catalogs

## Agent

- [?] Add retry to deploy test @agent(claude:0198f3ab)
  Done: PR #12.

# Done

## Feed

- [x] Old thing @done(2026-09-01)
";

    #[test]
    fn parses_items_headings_and_bodies() {
        let doc = parse(SAMPLE);
        let items: Vec<&Item> = doc.nodes.iter().filter_map(|n| match n {
            Node::Item(i) => Some(i),
            _ => None,
        }).collect();
        assert_eq!(items.len(), 6);
        assert_eq!(items[0].title, "Fix auth redirect loop");
        assert_eq!(items[0].state, State::Open);
        assert_eq!(items[0].body, vec!["Repro: bounces forever.", "See #142."]);
        assert_eq!(items[1].agent.as_ref().unwrap().to_string(), "claude:0198f3ab");
        assert_eq!(items[1].title, "Migrate CI");
        assert_eq!(items[5].done_date.as_deref(), Some("2026-09-01"));
        let headings: Vec<(u8, &str)> = doc.nodes.iter().filter_map(|n| match n {
            Node::Heading { level, text } => Some((*level, text.as_str())),
            _ => None,
        }).collect();
        assert_eq!(headings, vec![(1, "Feed"), (2, "Later"), (2, "Agent"), (1, "Done"), (2, "Feed")]);
    }

    #[test]
    fn unknown_state_char_becomes_raw() {
        let doc = parse("- [z] Weird\n");
        assert_eq!(doc.nodes, vec![Node::Raw("- [z] Weird".into())]);
    }

    #[test]
    fn blank_lines_are_raw() {
        let doc = parse("\n- [ ] A\n\n");
        assert!(matches!(doc.nodes[0], Node::Raw(_)));
        assert!(matches!(doc.nodes[2], Node::Raw(_)));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test parse`
Expected: FAIL (panics at `todo!`).

- [ ] **Step 3: Implement the parser**

```rust
use super::*;

pub fn parse(text: &str) -> Document {
    let mut nodes: Vec<Node> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            nodes.push(Node::Heading { level: 2, text: rest.trim().to_string() });
        } else if let Some(rest) = line.strip_prefix("# ") {
            nodes.push(Node::Heading { level: 1, text: rest.trim().to_string() });
        } else if let Some(item) = parse_item_line(line) {
            nodes.push(Node::Item(item));
        } else if line.starts_with("  ") && !line.trim().is_empty() {
            // Body line: attach to the most recent item if one directly precedes
            // (only Raw blank lines may not intervene — an item's body is contiguous).
            if let Some(Node::Item(item)) = nodes.last_mut() {
                item.body.push(line[2..].to_string());
                continue;
            }
            nodes.push(Node::Raw(line.to_string()));
        } else {
            nodes.push(Node::Raw(line.to_string()));
        }
    }
    Document { nodes }
}

fn parse_item_line(line: &str) -> Option<Item> {
    let rest = line.strip_prefix("- [")?;
    let mut chars = rest.chars();
    let state = State::from_char(chars.next()?)?;
    let rest = chars.as_str().strip_prefix("] ")?;

    let mut title = rest.trim_end().to_string();
    let mut agent = None;
    let mut done_date = None;
    // Strip trailing @key(value) tokens, rightmost first.
    loop {
        let t = title.trim_end();
        if let Some((head, tok)) = split_trailing_token(t) {
            match tok {
                Token::Agent(a) => agent = Some(a),
                Token::Done(d) => done_date = Some(d),
            }
            title = head.trim_end().to_string();
        } else {
            title = t.to_string();
            break;
        }
    }
    Some(Item { state, title, agent, done_date, body: Vec::new() })
}

enum Token {
    Agent(AgentRef),
    Done(String),
}

fn split_trailing_token(s: &str) -> Option<(&str, Token)> {
    let a = s.rfind("@agent(");
    let d = s.rfind("@done(");
    // Rightmost of the two token types — .or() would wrongly prefer @agent's
    // position anywhere in the string (bug caught in review: "@agent(x) @done(y)"
    // extracted neither token).
    let open = match (a, d) {
        (Some(x), Some(y)) => x.max(y),
        (x, y) => x.or(y)?,
    };
    let tail = &s[open..];
    let close = tail.find(')')?;
    if open + close + 1 != s.len() {
        return None; // token not at end of line
    }
    let inner = &tail[tail.find('(')? + 1..close];
    let tok = if tail.starts_with("@agent(") {
        Token::Agent(AgentRef::parse(inner)?)
    } else {
        Token::Done(inner.to_string())
    };
    Some((&s[..open], tok))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test parse`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/feed/parse.rs
git commit -m "feat: lossless feed parser"
```

---

### Task 4: Writer + round-trip + atomic save

**Files:**
- Create: `src/feed/write.rs`

- [ ] **Step 1: Write the failing tests**

```rust
use super::*;
use std::path::Path;

pub fn render(doc: &Document) -> String {
    todo!()
}

pub fn save_atomic(doc: &Document, path: &Path) -> anyhow::Result<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::parse::parse;

    #[test]
    fn round_trip_is_lossless() {
        let text = "\
# Feed

- [ ] Fix auth redirect loop
  Repro: bounces forever.
- [~] Migrate CI @agent(claude:0198f3ab)

Some stray prose the parser doesn't own.

## Agent

- [x] Old thing @done(2026-09-01)
- [x] @done(2026-09-02)
";
        assert_eq!(render(&parse(text)), text);
    }

    #[test]
    fn save_atomic_writes_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.md");
        let doc = parse("- [ ] A\n");
        save_atomic(&doc, &path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] A\n");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test write`
Expected: FAIL (panics at `todo!`).

- [ ] **Step 3: Implement**

```rust
use super::*;
use std::io::Write as _;
use std::path::Path;

pub fn render(doc: &Document) -> String {
    let mut out = String::new();
    for node in &doc.nodes {
        match node {
            Node::Heading { level, text } => {
                out.push_str(&"#".repeat(*level as usize));
                out.push(' ');
                out.push_str(text);
                out.push('\n');
            }
            Node::Raw(line) => {
                out.push_str(line);
                out.push('\n');
            }
            Node::Item(item) => {
                // No space after "]" when the title is empty — otherwise
                // "- [x] @done(...)" (empty title + token) round-trips with a
                // double space. Items can have empty titles (parser accepts
                // "- [x] " and token-only lines).
                out.push_str(&format!("- [{}]", item.state.to_char()));
                if !item.title.is_empty() {
                    out.push(' ');
                    out.push_str(&item.title);
                }
                if let Some(a) = &item.agent {
                    out.push_str(&format!(" @agent({a})"));
                }
                if let Some(d) = &item.done_date {
                    out.push_str(&format!(" @done({d})"));
                }
                out.push('\n');
                for b in &item.body {
                    out.push_str("  ");
                    out.push_str(b);
                    out.push('\n');
                }
            }
        }
    }
    out
}

pub fn save_atomic(doc: &Document, path: &Path) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(render(doc).as_bytes())?;
    tmp.persist(path)?;
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test write`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src/feed/write.rs
git commit -m "feat: lossless writer with atomic save"
```

---

### Task 5: Ops — sections, find, claim, add

**Files:**
- Create: `src/feed/ops.rs`

Concepts (spec §2): an item's **zone** comes from the headings before it — under `# Done` it's archived; under `## Agent` (outside Done) it's agent-created; anything else is human-created.

- [ ] **Step 1: Write the failing tests**

```rust
use super::*;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Human,
    Agent,
    Archive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    Human,
    Agent,
}

#[derive(Debug, Error)]
pub enum OpError {
    #[error("no item matches \"{0}\"")]
    NotFound(String),
    #[error("\"{0}\" is ambiguous: {}", .1.join(", "))]
    Ambiguous(String, Vec<String>),
    #[error("only the human closes human-created items (use review, or --as-human)")]
    NotAuthorised,
}

pub fn zone_of(doc: &Document, index: usize) -> Zone { todo!() }
pub fn find(doc: &Document, query: &str) -> Result<usize, OpError> { todo!() }
pub fn claim(doc: &mut Document, index: usize, agent: AgentRef) { todo!() }
pub fn add(doc: &mut Document, title: &str, body: &[String], zone: Zone) { todo!() }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::parse::parse;
    use crate::feed::write::render;

    const SAMPLE: &str = "\
# Feed

- [ ] Fix auth redirect loop
- [ ] Write onboarding doc

## Agent

- [ ] Add retry to deploy test

# Done

## Feed

- [x] Old fixed thing @done(2026-09-01)
";

    #[test]
    fn zones_follow_headings() {
        let doc = parse(SAMPLE);
        let idx: Vec<(usize, Zone)> = doc.nodes.iter().enumerate()
            .filter(|(_, n)| matches!(n, Node::Item(_)))
            .map(|(i, _)| (i, zone_of(&doc, i)))
            .collect();
        assert_eq!(idx[0].1, Zone::Human);
        assert_eq!(idx[1].1, Zone::Human);
        assert_eq!(idx[2].1, Zone::Agent);
        assert_eq!(idx[3].1, Zone::Archive);
    }

    #[test]
    fn find_matches_substring_case_insensitive_active_only() {
        let doc = parse(SAMPLE);
        let i = find(&doc, "AUTH").unwrap();
        assert!(matches!(&doc.nodes[i], Node::Item(it) if it.title.contains("auth")));
        assert!(matches!(find(&doc, "old fixed"), Err(OpError::NotFound(_))));
        assert!(matches!(find(&doc, "o"), Err(OpError::Ambiguous(..))));
    }

    #[test]
    fn claim_sets_state_and_agent() {
        let mut doc = parse(SAMPLE);
        let i = find(&doc, "auth").unwrap();
        claim(&mut doc, i, AgentRef::parse("claude:abc").unwrap());
        assert!(render(&doc).contains("- [~] Fix auth redirect loop @agent(claude:abc)"));
    }

    #[test]
    fn add_mine_appends_to_first_human_section_end() {
        let mut doc = parse(SAMPLE);
        add(&mut doc, "New human task", &["ctx".into()], Zone::Human);
        let out = render(&doc);
        let human_pos = out.find("New human task").unwrap();
        assert!(human_pos < out.find("## Agent").unwrap());
        assert!(out.contains("- [ ] New human task\n  ctx\n"));
    }

    #[test]
    fn add_agent_appends_to_agent_section_creating_it() {
        let mut doc = parse("# Feed\n\n- [ ] A\n");
        add(&mut doc, "Agent task", &[], Zone::Agent);
        let out = render(&doc);
        assert!(out.contains("## Agent\n\n- [ ] Agent task\n"));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test ops`
Expected: FAIL (panics at `todo!`).

- [ ] **Step 3: Implement**

```rust
pub fn zone_of(doc: &Document, index: usize) -> Zone {
    let mut zone = Zone::Human;
    for node in &doc.nodes[..=index] {
        if let Node::Heading { level, text } = node {
            match level {
                1 if text.eq_ignore_ascii_case("Done") => zone = Zone::Archive,
                1 => zone = Zone::Human,
                2 if zone != Zone::Archive => {
                    zone = if text.eq_ignore_ascii_case("Agent") { Zone::Agent } else { Zone::Human };
                }
                _ => {}
            }
        }
    }
    zone
}

pub fn find(doc: &Document, query: &str) -> Result<usize, OpError> {
    let q = query.to_lowercase();
    let matches: Vec<usize> = doc.nodes.iter().enumerate()
        .filter_map(|(i, n)| match n {
            Node::Item(it) if zone_of(doc, i) != Zone::Archive
                && it.title.to_lowercase().contains(&q) => Some(i),
            _ => None,
        })
        .collect();
    match matches.len() {
        0 => Err(OpError::NotFound(query.to_string())),
        1 => Ok(matches[0]),
        _ => Err(OpError::Ambiguous(
            query.to_string(),
            matches.iter().map(|&i| match &doc.nodes[i] {
                Node::Item(it) => it.title.clone(),
                _ => unreachable!(),
            }).collect(),
        )),
    }
}

pub fn claim(doc: &mut Document, index: usize, agent: AgentRef) {
    if let Node::Item(it) = &mut doc.nodes[index] {
        it.state = State::InProgress;
        it.agent = Some(agent);
    }
}

pub fn add(doc: &mut Document, title: &str, body: &[String], zone: Zone) {
    let item = Node::Item(Item {
        state: State::Open,
        title: title.to_string(),
        agent: None,
        done_date: None,
        // Filter empty lines per the Item.body invariant (empty body lines
        // break lossless round-tripping).
        body: body.iter().filter(|l| !l.is_empty()).cloned().collect(),
    });
    let insert_at = match zone {
        Zone::Human => end_of_first_human_section(doc),
        Zone::Agent | Zone::Archive => {
            let at = agent_section_end(doc);
            match at {
                Some(i) => i,
                None => {
                    // Create "## Agent" just before "# Done" (or at EOF).
                    let done = doc.nodes.iter().position(|n|
                        matches!(n, Node::Heading { level: 1, text } if text.eq_ignore_ascii_case("Done")));
                    let at = done.unwrap_or(doc.nodes.len());
                    doc.nodes.insert(at, Node::Raw(String::new()));
                    doc.nodes.insert(at, Node::Heading { level: 2, text: "Agent".into() });
                    doc.nodes.insert(at, Node::Raw(String::new()));
                    at + 3
                }
            }
        }
    };
    doc.nodes.insert(insert_at, item);
}

/// Insertion index for a new human item: just past the last item of the first
/// non-empty human region, or — when no human items exist yet — directly
/// before the boundary heading (`## Agent` / `# Done`), stepping back over a
/// single preceding blank line; end of document when there is no boundary.
/// (An earlier draft searched globally for the first item as a fallback,
/// which inserted into ## Agent / # Done on post-sweep feeds — review-caught.)
fn end_of_first_human_section(doc: &Document) -> usize {
    let mut last_item_end = 0usize;
    let mut boundary: Option<usize> = None;
    for (i, n) in doc.nodes.iter().enumerate() {
        match n {
            Node::Heading { level: 2, text } if text.eq_ignore_ascii_case("Agent") => {
                boundary = Some(i);
                break;
            }
            Node::Heading { level: 1, text } if text.eq_ignore_ascii_case("Done") => {
                boundary = Some(i);
                break;
            }
            Node::Heading { level: 2, .. } if last_item_end > 0 => {
                boundary = Some(i);
                break;
            }
            Node::Item(_) => last_item_end = i + 1,
            _ => {}
        }
    }
    if last_item_end > 0 {
        return last_item_end;
    }
    match boundary {
        Some(b) => {
            if b > 0 && matches!(&doc.nodes[b - 1], Node::Raw(s) if s.is_empty()) {
                b - 1
            } else {
                b
            }
        }
        None => doc.nodes.len(),
    }
}

fn agent_section_end(doc: &Document) -> Option<usize> {
    let start = doc.nodes.iter().position(|n|
        matches!(n, Node::Heading { level: 2, text } if text.eq_ignore_ascii_case("Agent")))?;
    if zone_of(doc, start) == Zone::Archive {
        return None;
    }
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

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test ops`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add src/feed/ops.rs
git commit -m "feat: zones, find, claim, add operations"
```

---

### Task 6: Ops — state changes, authority, sweep

**Files:**
- Modify: `src/feed/ops.rs`

- [ ] **Step 1: Write the failing tests** (append to `ops.rs` tests module)

```rust
    #[test]
    fn review_sets_question_state() {
        let mut doc = parse(SAMPLE);
        let i = find(&doc, "auth").unwrap();
        set_state(&mut doc, i, State::Review, Authority::Agent).unwrap();
        assert!(render(&doc).contains("- [?] Fix auth redirect loop"));
    }

    #[test]
    fn agent_cannot_close_human_item() {
        let mut doc = parse(SAMPLE);
        let i = find(&doc, "auth").unwrap();
        let err = set_state(&mut doc, i, State::Done, Authority::Agent).unwrap_err();
        assert!(matches!(err, OpError::NotAuthorised));
    }

    #[test]
    fn agent_can_close_agent_item_and_human_can_close_anything() {
        let mut doc = parse(SAMPLE);
        let i = find(&doc, "deploy").unwrap();
        set_state(&mut doc, i, State::Done, Authority::Agent).unwrap();
        let i = find(&doc, "auth").unwrap();
        set_state(&mut doc, i, State::Done, Authority::Human).unwrap();
        let out = render(&doc);
        assert!(out.contains("- [x] Add retry to deploy test"));
        assert!(out.contains("- [x] Fix auth redirect loop"));
    }

    #[test]
    fn sweep_archives_human_done_and_deletes_agent_done() {
        let text = "\
# Feed

- [x] Shipped thing
  Evidence: PR #9.
- [ ] Still open
- [?] Awaiting review

## Agent

- [x] Agent chore

# Done

## Feed

- [x] Older thing @done(2026-09-01)
";
        let mut doc = parse(text);
        sweep(&mut doc, "2026-09-11");
        let out = render(&doc);
        // Human [x] moved to Done under mirrored section, body kept, stamped.
        assert!(out.contains("# Done"));
        assert!(out.contains("- [x] Shipped thing @done(2026-09-11)\n  Evidence: PR #9.\n"));
        // Moved, not copied; [ ] and [?] untouched; agent [x] deleted.
        assert_eq!(out.matches("Shipped thing").count(), 1);
        assert!(out.find("Shipped thing").unwrap() > out.find("# Done").unwrap());
        assert!(out.contains("- [ ] Still open"));
        assert!(out.contains("- [?] Awaiting review"));
        assert!(!out.contains("Agent chore"));
        assert!(out.contains("@done(2026-09-01)"));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test ops`
Expected: FAIL — `set_state`/`sweep` not defined.

- [ ] **Step 3: Implement**

```rust
pub fn set_state(doc: &mut Document, index: usize, state: State, by: Authority) -> Result<(), OpError> {
    if state == State::Done && by == Authority::Agent && zone_of(doc, index) == Zone::Human {
        return Err(OpError::NotAuthorised);
    }
    if let Node::Item(it) = &mut doc.nodes[index] {
        it.state = state;
    }
    Ok(())
}

/// Human section name an active item sits under (for mirroring in Done).
fn section_name(doc: &Document, index: usize) -> String {
    let mut name = "Feed".to_string();
    for node in &doc.nodes[..index] {
        match node {
            Node::Heading { level: 1, text } => name = text.clone(),
            Node::Heading { level: 2, text } => name = text.clone(),
            _ => {}
        }
    }
    name
}

pub fn sweep(doc: &mut Document, today: &str) {
    // Collect indices of active [x] items, back to front so removal is stable.
    let done_items: Vec<usize> = doc.nodes.iter().enumerate()
        .filter_map(|(i, n)| match n {
            Node::Item(it) if it.state == State::Done && zone_of(doc, i) != Zone::Archive => Some(i),
            _ => None,
        })
        .collect();

    let mut archived: Vec<(String, Item)> = Vec::new();
    for &i in done_items.iter().rev() {
        let zone = zone_of(doc, i);
        let section = section_name(doc, i);
        if let Node::Item(item) = doc.nodes.remove(i) {
            if zone == Zone::Human {
                let mut item = item;
                item.done_date = Some(today.to_string());
                archived.push((section, item));
            }
            // Agent zone: dropped.
        }
    }
    archived.reverse();

    if archived.is_empty() {
        return;
    }

    // Ensure "# Done" exists at EOF.
    let done_at = doc.nodes.iter().position(|n|
        matches!(n, Node::Heading { level: 1, text } if text.eq_ignore_ascii_case("Done")));
    let done_at = done_at.unwrap_or_else(|| {
        doc.nodes.push(Node::Raw(String::new()));
        doc.nodes.push(Node::Heading { level: 1, text: "Done".into() });
        doc.nodes.len() - 1
    });

    for (section, item) in archived {
        // Find or create the mirrored "## <section>" inside Done.
        let mut insert_at = doc.nodes.len();
        let mut found = false;
        let mut i = done_at + 1;
        while i < doc.nodes.len() {
            match &doc.nodes[i] {
                Node::Heading { level: 2, text } if text.eq_ignore_ascii_case(&section) => {
                    found = true;
                    // Insert after the last node of this subsection.
                    let mut j = i + 1;
                    while j < doc.nodes.len() && !matches!(doc.nodes[j], Node::Heading { .. }) {
                        j += 1;
                    }
                    insert_at = j;
                    break;
                }
                _ => i += 1,
            }
        }
        if !found {
            doc.nodes.push(Node::Raw(String::new()));
            doc.nodes.push(Node::Heading { level: 2, text: section });
            doc.nodes.push(Node::Raw(String::new()));
            insert_at = doc.nodes.len();
        }
        doc.nodes.insert(insert_at, Node::Item(item));
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test ops`
Expected: all ops tests pass (9 total).

- [ ] **Step 5: Commit**

```bash
git add src/feed/ops.rs
git commit -m "feat: state changes with completion authority, sweep with archive"
```

---

### Task 7: Feed path resolution

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs` (add `mod config;`)

Resolution order (spec §1 + #9): explicit `--file` > `FEEDR_FEED` env > `feed_path` in `~/.config/herdr-feedr/config.toml` > default `~/.config/herdr-feedr/feed.md`. (Plan 3 may relocate the config file to herdr's plugin-config dir; the resolution function is the single place to change.)

- [ ] **Step 1: Write the failing test**

```rust
use std::path::PathBuf;

pub fn resolve_feed_path(
    cli_file: Option<PathBuf>,
    env_file: Option<PathBuf>,
    config_dir: &std::path::Path,
) -> PathBuf {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_cli_env_config_default() {
        let dir = tempfile::tempdir().unwrap();
        // Default when nothing set:
        assert_eq!(
            resolve_feed_path(None, None, dir.path()),
            dir.path().join("feed.md")
        );
        // Config file wins over default:
        std::fs::write(dir.path().join("config.toml"), "feed_path = \"/tmp/custom.md\"\n").unwrap();
        assert_eq!(resolve_feed_path(None, None, dir.path()), PathBuf::from("/tmp/custom.md"));
        // Env wins over config:
        assert_eq!(
            resolve_feed_path(None, Some("/tmp/env.md".into()), dir.path()),
            PathBuf::from("/tmp/env.md")
        );
        // CLI wins over all:
        assert_eq!(
            resolve_feed_path(Some("/tmp/cli.md".into()), Some("/tmp/env.md".into()), dir.path()),
            PathBuf::from("/tmp/cli.md")
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test config`
Expected: FAIL (panics at `todo!`).

- [ ] **Step 3: Implement**

```rust
use serde::Deserialize;

#[derive(Deserialize, Default)]
struct FileConfig {
    feed_path: Option<PathBuf>,
}

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
    let cfg: FileConfig = std::fs::read_to_string(config_dir.join("config.toml"))
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default();
    cfg.feed_path.unwrap_or_else(|| config_dir.join("feed.md"))
}

pub fn default_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("herdr-feedr")
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test config`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "feat: feed path resolution (flag > env > config > default)"
```

---

### Task 8: CLI

**Files:**
- Create: `src/cli.rs`
- Modify: `src/main.rs`
- Test: `tests/cli.rs`

- [ ] **Step 1: Write the failing end-to-end tests** (`tests/cli.rs`)

```rust
use assert_cmd::Command;
use predicates::str::contains;

fn feedr(feed: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("feedr").unwrap();
    cmd.arg("--file").arg(feed);
    cmd
}

fn seed(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("feed.md");
    std::fs::write(&path, "\
# Feed

- [ ] Fix auth redirect loop
  Repro: bounces forever.
- [ ] Write onboarding doc
").unwrap();
    path
}

#[test]
fn list_shows_active_items_with_states() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed).arg("list").assert().success()
        .stdout(contains("[ ] Fix auth redirect loop"))
        .stdout(contains("[ ] Write onboarding doc"));
}

#[test]
fn show_prints_title_and_body() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed).args(["show", "auth"]).assert().success()
        .stdout(contains("Fix auth redirect loop"))
        .stdout(contains("Repro: bounces forever."));
}

#[test]
fn claim_then_review_then_human_done() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed).args(["claim", "auth", "--agent", "claude:abc123"]).assert().success();
    assert!(std::fs::read_to_string(&feed).unwrap()
        .contains("- [~] Fix auth redirect loop @agent(claude:abc123)"));

    feedr(&feed).args(["review", "auth"]).assert().success();
    assert!(std::fs::read_to_string(&feed).unwrap().contains("- [?] Fix auth"));

    // Agent may not close a human item:
    feedr(&feed).args(["done", "auth"]).assert().failure()
        .stderr(contains("only the human"));
    // Human may:
    feedr(&feed).args(["done", "auth", "--as-human"]).assert().success();
    assert!(std::fs::read_to_string(&feed).unwrap().contains("- [x] Fix auth"));
}

#[test]
fn add_and_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed).args(["add", "--agent-owned", "Investigate flaky test", "--body", "Seen in CI run 42."])
        .assert().success();
    let text = std::fs::read_to_string(&feed).unwrap();
    assert!(text.contains("## Agent"));
    assert!(text.contains("- [ ] Investigate flaky test\n  Seen in CI run 42."));

    feedr(&feed).args(["done", "onboarding", "--as-human"]).assert().success();
    feedr(&feed).arg("sweep").assert().success();
    let text = std::fs::read_to_string(&feed).unwrap();
    assert!(text.contains("# Done"));
    assert!(text.contains("- [x] Write onboarding doc @done("));
}

#[test]
fn claim_requires_agent_ref() {
    let dir = tempfile::tempdir().unwrap();
    let feed = seed(dir.path());
    feedr(&feed).args(["claim", "auth"]).assert().failure();
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --test cli`
Expected: FAIL — subcommands not implemented.

- [ ] **Step 3: Implement `src/cli.rs`**

```rust
use crate::config;
use crate::feed::{ops, parse::parse, write, AgentRef, Document, State};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "feedr", about = "The feed rack for your herd")]
pub struct Cli {
    /// Feed file (overrides FEEDR_FEED and config)
    #[arg(long, global = true)]
    file: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List active items
    List,
    /// Show one item with its body
    Show { item: String },
    /// Claim an item: [~] + @agent tag
    Claim {
        item: String,
        /// Claiming agent as kind:session-id, e.g. claude:0198f3ab
        #[arg(long)]
        agent: String,
    },
    /// Add an item
    Add {
        title: String,
        /// Body/context lines
        #[arg(long)]
        body: Vec<String>,
        /// Put it in the reserved "## Agent" section (agent-initiated work)
        #[arg(long)]
        agent_owned: bool,
        /// Target a named human section instead of the first one
        #[arg(long, conflicts_with = "agent_owned")]
        section: Option<String>,
    },
    /// Mark an item awaiting review: [?]
    Review { item: String },
    /// Close an item: [x]
    Done {
        item: String,
        /// Assert human authority (the sidebar and you use this; agents must not)
        #[arg(long)]
        as_human: bool,
    },
    /// Archive human [x] items under "# Done"; delete agent [x] items
    Sweep,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let path = config::resolve_feed_path(
        cli.file.clone(),
        std::env::var_os("FEEDR_FEED").map(PathBuf::from),
        &config::default_config_dir(),
    );
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc = parse(&text);

    match cli.command {
        Cmd::List => {
            for (i, node) in doc.nodes.iter().enumerate() {
                match node {
                    crate::feed::Node::Heading { level, text }
                        if ops::zone_of(&doc, i) != ops::Zone::Archive && *level == 2 =>
                        println!("{text}:"),
                    crate::feed::Node::Item(it) if ops::zone_of(&doc, i) != ops::Zone::Archive => {
                        let agent = it.agent.as_ref().map(|a| format!("  @{a}")).unwrap_or_default();
                        println!("[{}] {}{agent}", it.state.to_char(), it.title);
                    }
                    _ => {}
                }
            }
        }
        Cmd::Show { item } => {
            let i = ops::find(&doc, &item)?;
            if let crate::feed::Node::Item(it) = &doc.nodes[i] {
                println!("[{}] {}", it.state.to_char(), it.title);
                for b in &it.body {
                    println!("  {b}");
                }
            }
        }
        Cmd::Claim { item, agent } => {
            let a = AgentRef::parse(&agent)
                .with_context(|| format!("--agent must be kind:id, got \"{agent}\""))?;
            let i = ops::find(&doc, &item)?;
            ops::claim(&mut doc, i, a);
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Add { title, body, agent_owned, section } => {
            if section.is_some() {
                bail!("--section: not implemented in v0.1, edit the file or omit");
            }
            let zone = if agent_owned { ops::Zone::Agent } else { ops::Zone::Human };
            ops::add(&mut doc, &title, &body, zone);
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Review { item } => {
            let i = ops::find(&doc, &item)?;
            ops::set_state(&mut doc, i, State::Review, ops::Authority::Agent)?;
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Done { item, as_human } => {
            let i = ops::find(&doc, &item)?;
            let by = if as_human { ops::Authority::Human } else { ops::Authority::Agent };
            ops::set_state(&mut doc, i, State::Done, by)?;
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Sweep => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            ops::sweep(&mut doc, &today);
            write::save_atomic(&doc, &path)?;
        }
    }
    Ok(())
}
```

Note: `AgentRef::parse` returns `Option`; `.with_context` needs `ok_or_else` — use:

```rust
let a = AgentRef::parse(&agent)
    .ok_or_else(|| anyhow::anyhow!("--agent must be kind:id, got \"{agent}\""))?;
```

And `src/main.rs` becomes:

```rust
mod cli;
mod config;
mod feed;

fn main() {
    if let Err(e) = cli::run() {
        eprintln!("feedr: {e}");
        std::process::exit(1);
    }
}
```

(`OpError` must convert into `anyhow::Error` automatically — it does, via `thiserror`.)

- [ ] **Step 4: Run the full suite**

Run: `cargo test`
Expected: all unit + CLI tests pass.

- [ ] **Step 5: Commit**

```bash
git add src tests
git commit -m "feat: feedr CLI — list/show/claim/add/review/done/sweep"
```

---

### Task 9: Wrap-up

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Run lint + full suite once more**

Run: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`
Expected: clean. Fix anything that isn't (fmt/clippy autofixes are fine).

- [ ] **Step 2: Update README status line**

Change the **Status** line in `README.md` to:

```markdown
**Status: core + CLI implemented** (`cargo install --path .` → `feedr --help`). Sidebar TUI and herdr plugin packaging are next; the v1 spec is in [docs/SPEC.md](docs/SPEC.md).
```

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: README status — core + CLI implemented"
```

---

## Self-review notes

- Spec coverage (this plan's scope): §1 format ✔ (Tasks 2–4), §2 authority ✔ (Task 6), §2a sweep ✔ (Task 6), §4 CLI ✔ (Task 8), §6 concurrency ✔ (atomic save Task 4; every command re-reads fresh in Task 8), §9 path resolution ✔ (Task 7). §3 sidebar, §5 skill delivery, §7 socket, §8 packaging → Plans 2–3, deliberately.
- `feedr add --section` is declared but bails in v0.1 — an honest error, not silent scope; the TUI modal (Plan 2) is the section-picker path, and the flag completes then.
- Type names consistent across tasks: `Document/Node/Item/State/AgentRef/Zone/Authority/OpError` all defined in Tasks 2 and 5 and only referenced afterwards.
