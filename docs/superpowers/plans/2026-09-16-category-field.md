# Category Field Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Category field (topmost, with suggestion dropdown) to the create/edit modal; categories are `##` sections, so the list view groups for free.

**Architecture:** Categories reuse the feed file's `##` headings (spec: `docs/superpowers/specs/2026-09-16-category-field-design.md`). New ops (`ensure_section`, `move_to_section`, `section_names`, `item_section`) extend `src/feed/ops.rs`; the modal (`src/tui/modal.rs`) gains a Category `TextArea` + dropdown sub-state; `App::save_modal` routes create/move through the existing `with_feed` read-fresh → atomic-write path. No parser/writer changes.

**Tech Stack:** Rust, ratatui, tui-textarea, crossterm. Tests: `cargo test` (unit tests live in each module's `#[cfg(test)] mod tests`).

**Conventions:** All paths relative to repo root (`herdr-feedr-sidebar` worktree, branch `feat/sidebar-tui`). Ops tests build docs with `parse("...markdown...")` and assert on `render(&doc)` output — follow that style. Run `cargo fmt` and `cargo clippy --all-targets` before each commit.

---

### Task 1: Reserved names + `section_names` (ops)

**Files:**
- Modify: `src/feed/ops.rs` (new fns near `named_section_end`, ~line 200; tests at bottom of the file's `mod tests`)

- [ ] **Step 1: Write the failing tests** (append inside `mod tests` in `src/feed/ops.rs`)

```rust
#[test]
fn section_names_collects_active_and_archive_dedup_case_insensitive() {
    let doc = parse(
        "# Feed\n\n- [ ] A\n\n## Work\n\n- [ ] W\n\n## Later\n\n## Agent\n\n- [ ] G\n\n# Done\n\n## work\n\n- [x] Old @done(2026-09-01)\n\n## Chores\n\n- [x] C @done(2026-09-01)\n",
    );
    // First-seen casing wins; "Agent" excluded; archive "Chores" included.
    assert_eq!(section_names(&doc), vec!["Work", "Later", "Chores"]);
}

#[test]
fn section_names_excludes_reserved() {
    let doc = parse("# Feed\n\n## Agent\n\n# Done\n\n## Feed\n\n- [x] Old @done(2026-09-01)\n");
    assert!(section_names(&doc).is_empty());
    assert!(is_reserved_section("agent"));
    assert!(is_reserved_section("DONE"));
    assert!(is_reserved_section("Feed"));
    assert!(!is_reserved_section("Work"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test feed::ops::tests::section_names -- --nocapture`
Expected: compile error — `section_names` / `is_reserved_section` not found.

- [ ] **Step 3: Implement** (in `src/feed/ops.rs`, after `named_section_end`)

```rust
/// Section names that can never be categories: "Agent" is the agents' zone,
/// "Done" the archive, "Feed" the archive's mirror name for uncategorized
/// items (spec 2026-09-16 §1).
const RESERVED_SECTIONS: [&str; 3] = ["agent", "done", "feed"];

pub fn is_reserved_section(name: &str) -> bool {
    RESERVED_SECTIONS
        .iter()
        .any(|r| name.eq_ignore_ascii_case(r))
}

/// Category suggestions for the modal: every `##` heading in the file —
/// active sections plus the `# Done` archive's mirrored names ("categories
/// used in the past") — first-seen casing, case-insensitively deduplicated,
/// reserved names excluded.
pub fn section_names(doc: &Document) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for n in &doc.nodes {
        if let Node::Heading { level: 2, text } = n {
            if is_reserved_section(text) {
                continue;
            }
            if !names.iter().any(|s| s.eq_ignore_ascii_case(text)) {
                names.push(text.clone());
            }
        }
    }
    names
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test feed::ops::tests::section_names`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src/feed/ops.rs
git commit -m "feat: reserved-name guard and section_names for category suggestions"
```

---

### Task 2: `OpError::Reserved` + `ensure_section` (ops)

**Files:**
- Modify: `src/feed/ops.rs` (`OpError` enum ~line 17; new fn after `section_names`; tests at bottom)

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn ensure_section_finds_existing_case_insensitive() {
    let mut doc = parse("# Feed\n\n## Work\n\n- [ ] W\n\n## Agent\n");
    let at = ensure_section(&mut doc, "work").unwrap();
    doc.nodes.insert(
        at,
        Node::Item(Item {
            state: State::Open,
            title: "New".into(),
            agent: None,
            done_date: None,
            body: Vec::new(),
        }),
    );
    assert!(render(&doc).contains("- [ ] W\n- [ ] New\n"), "got:\n{}", render(&doc));
}

#[test]
fn ensure_section_creates_before_agent_then_done_then_eof() {
    // Before ## Agent — and an item inserted at the returned index renders
    // with single blank lines on both sides (no double blank, no gluing):
    let mut doc = parse("# Feed\n\n- [ ] A\n\n## Agent\n\n- [ ] G\n");
    let at = ensure_section(&mut doc, "Work").unwrap();
    doc.nodes.insert(
        at,
        Node::Item(Item {
            state: State::Open,
            title: "X".into(),
            agent: None,
            done_date: None,
            body: Vec::new(),
        }),
    );
    let out = render(&doc);
    assert!(
        out.contains("- [ ] A\n\n## Work\n\n- [ ] X\n\n## Agent\n"),
        "got:\n{out}"
    );
    // No ## Agent — before # Done:
    let mut doc = parse("# Feed\n\n- [ ] A\n\n# Done\n\n## Feed\n\n- [x] Old @done(2026-09-01)\n");
    ensure_section(&mut doc, "Work").unwrap();
    let out = render(&doc);
    assert!(out.find("## Work").unwrap() < out.find("# Done").unwrap(), "got:\n{out}");
    // Neither — end of file:
    let mut doc = parse("# Feed\n\n- [ ] A\n");
    let at = ensure_section(&mut doc, "Work").unwrap();
    assert_eq!(at, doc.nodes.len());
    assert!(render(&doc).ends_with("## Work\n\n"), "got:\n{}", render(&doc));
}

#[test]
fn ensure_section_rejects_reserved_and_ignores_archive_sections() {
    let mut doc = parse("# Feed\n\n# Done\n\n## Chores\n\n- [x] C @done(2026-09-01)\n");
    assert!(matches!(ensure_section(&mut doc, "Agent"), Err(OpError::Reserved(_))));
    // "Chores" exists only in the archive → a NEW active section is created:
    ensure_section(&mut doc, "Chores").unwrap();
    let out = render(&doc);
    assert!(out.find("## Chores").unwrap() < out.find("# Done").unwrap(), "got:\n{out}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test feed::ops::tests::ensure_section`
Expected: compile error — `ensure_section` / `OpError::Reserved` not found.

- [ ] **Step 3: Implement**

Add to `OpError`:

```rust
    #[error("\"{0}\" is a reserved section")]
    Reserved(String),
```

Add after `section_names` (the blank/heading/blank insert mirrors the `## Agent` auto-create in `add`, ~line 107):

```rust
/// Find the active `## name` section (case-insensitive; archive subsections
/// never match) or create it at the end of the human zone — before
/// `## Agent` if present, else before `# Done`, else at EOF. Returns the
/// insertion index for a new item at the section's end.
pub fn ensure_section(doc: &mut Document, name: &str) -> Result<usize, OpError> {
    if is_reserved_section(name) {
        return Err(OpError::Reserved(name.to_string()));
    }
    if let Some(end) = named_section_end(doc, name) {
        return Ok(end);
    }
    let mut at = doc
        .nodes
        .iter()
        .position(|n| match n {
            Node::Heading { level: 2, text } => text.eq_ignore_ascii_case("Agent"),
            Node::Heading { level: 1, text } => text.eq_ignore_ascii_case("Done"),
            _ => false,
        })
        .unwrap_or(doc.nodes.len());
    // Step back over a single blank preceding the boundary (the same dance as
    // end_of_first_human_section) so the new section slots between the last
    // item's blank and the boundary's own blank — otherwise the file gains a
    // double blank line and the first item glues against the boundary heading.
    if at > 0 && matches!(&doc.nodes[at - 1], Node::Raw(s) if s.is_empty()) {
        at -= 1;
    }
    doc.nodes.insert(at, Node::Raw(String::new()));
    doc.nodes.insert(
        at,
        Node::Heading {
            level: 2,
            text: name.to_string(),
        },
    );
    doc.nodes.insert(at, Node::Raw(String::new()));
    Ok(at + 3)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test feed::ops::tests::ensure_section`
Expected: 3 passed. (If the EOF assertion fails on exact trailing blanks, match the actual render — the invariant that matters is a `## Work` heading after the items; adjust the assertion to `out.contains("## Work")` plus position checks, not the impl.)

- [ ] **Step 5: Commit**

```bash
git add src/feed/ops.rs
git commit -m "feat: ensure_section creates categories at the end of the human zone"
```

---

### Task 3: `add_in_section` creates missing sections (ops + CLI behavior)

**Files:**
- Modify: `src/feed/ops.rs:224-246` (`add_in_section`), test `add_in_section_appends_to_named_human_section` (~line 763)

Note: `src/cli.rs:161` (`feedr add --section`) calls this — behavior change is intended: `--section New` now creates the section instead of erroring, and `--section Agent` now errors `Reserved` (agents use `--agent-owned` for the Agent zone).

- [ ] **Step 1: Update the test** (replace the existing `add_in_section_appends_to_named_human_section` error assertions)

```rust
#[test]
fn add_in_section_appends_to_named_human_section() {
    let mut doc2 = parse("# Feed\n\n- [ ] A\n\n## Later\n\n- [ ] L1\n\n## Agent\n\n- [ ] G\n");
    add_in_section(&mut doc2, "L2", &["ctx".into()], "Later").unwrap();
    let out = render(&doc2);
    assert!(out.contains("- [ ] L1\n- [ ] L2\n  ctx\n"), "got:\n{out}");
    // A missing section is created (before ## Agent), not an error:
    add_in_section(&mut doc2, "X", &[], "Fresh").unwrap();
    let out = render(&doc2);
    assert!(out.find("## Fresh").unwrap() < out.find("## Agent").unwrap(), "got:\n{out}");
    assert!(out.contains("## Fresh\n\n- [ ] X\n"), "got:\n{out}");
    // Reserved names error ("Feed" exists only under # Done in SAMPLE):
    let mut doc = parse(SAMPLE);
    assert!(matches!(
        add_in_section(&mut doc, "X", &[], "Feed"),
        Err(OpError::Reserved(_))
    ));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test feed::ops::tests::add_in_section`
Expected: FAIL — `add_in_section(.., "Fresh")` returns `NotFound`.

- [ ] **Step 3: Reimplement `add_in_section`**

```rust
/// Add an open item at the end of the named `##` section, creating the
/// section at the end of the human zone when it doesn't exist (archive
/// sections never match). Reserved names error.
pub fn add_in_section(
    doc: &mut Document,
    title: &str,
    body: &[String],
    section: &str,
) -> Result<(), OpError> {
    let end = ensure_section(doc, section)?;
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
```

- [ ] **Step 4: Run the whole suite** (CLI tests may assert the old error)

Run: `cargo test`
Expected: PASS. If a CLI/integration test asserts `add --section` errors on a missing section, update it to assert creation instead.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: add_in_section creates missing sections (feedr add --section too)"
```

---

### Task 4: `item_section` accessor (ops) + Task-3 review follow-ups

**Files:**
- Modify: `src/feed/ops.rs` (near the private `section_name`, ~line 265; tests at bottom)
- Modify: `src/cli.rs` (~line 42, `--section` help text)

Review follow-ups folded in (from Task 3's code review):
- `ensure_section` gains an empty-name guard: trim the name at the top and use the
  trimmed name throughout; a trimmed-empty name returns a new
  `#[error("section name required")] EmptyName` variant on `OpError`. Test: creating
  with `"  "` errors `EmptyName`, doc untouched.
- `src/cli.rs` `--section` help text updated to reflect create-on-missing, e.g.
  "Target a named section, creating it if missing (reserved: Agent/Done/Feed)".

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn item_section_names_the_enclosing_level2_heading() {
    let doc = parse("# Feed\n\n- [ ] A\n\n## Work\n\n- [ ] W\n\n## Agent\n\n- [ ] G\n");
    let a = find(&doc, "A").unwrap();
    let w = find(&doc, "W").unwrap();
    let g = find(&doc, "G").unwrap();
    assert_eq!(item_section(&doc, a), None); // uncategorized
    assert_eq!(item_section(&doc, w).as_deref(), Some("Work"));
    assert_eq!(item_section(&doc, g).as_deref(), Some("Agent"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test feed::ops::tests::item_section`
Expected: compile error — `item_section` not found.

- [ ] **Step 3: Implement**

```rust
/// The `##` section an item sits under, or None when it's uncategorized
/// (directly under a `#` heading). Prefills the modal's Category field —
/// unlike the private `section_name` (archive mirroring), this does not
/// fall back to the level-1 heading's name.
pub fn item_section(doc: &Document, index: usize) -> Option<String> {
    let mut current: Option<String> = None;
    for node in &doc.nodes[..index] {
        match node {
            Node::Heading { level: 1, .. } => current = None,
            Node::Heading { level: 2, text } => current = Some(text.clone()),
            _ => {}
        }
    }
    current
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test feed::ops::tests::item_section`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/feed/ops.rs
git commit -m "feat: item_section reports an item's enclosing category"
```

---

### Task 5: `move_to_section` (ops)

**Files:**
- Modify: `src/feed/ops.rs` (after `add_in_section`; tests at bottom)

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn move_to_section_preserves_state_body_and_tokens() {
    let mut doc = parse("# Feed\n\n- [~] A @agent(claude:abc)\n  ctx line\n\n## Work\n\n- [ ] W\n");
    let i = find(&doc, "A").unwrap();
    move_to_section(&mut doc, i, Some("Work")).unwrap();
    let out = render(&doc);
    assert!(out.contains("- [ ] W\n- [~] A @agent(claude:abc)\n  ctx line\n"), "got:\n{out}");
}

#[test]
fn move_to_section_creates_target_and_keeps_emptied_heading() {
    let mut doc = parse("# Feed\n\n## Work\n\n- [ ] Only\n\n## Agent\n");
    let i = find(&doc, "Only").unwrap();
    move_to_section(&mut doc, i, Some("Chores")).unwrap();
    let out = render(&doc);
    assert!(out.contains("## Work"), "emptied heading must survive:\n{out}");
    assert!(out.contains("## Chores\n\n- [ ] Only\n"), "got:\n{out}");
    assert!(out.find("## Chores").unwrap() < out.find("## Agent").unwrap(), "got:\n{out}");
}

#[test]
fn move_to_none_lands_in_first_human_section_and_reserved_errors() {
    let mut doc = parse("# Feed\n\n- [ ] A\n\n## Work\n\n- [ ] W\n");
    let w = find(&doc, "W").unwrap();
    move_to_section(&mut doc, w, None).unwrap();
    assert!(render(&doc).contains("- [ ] A\n- [ ] W\n"), "got:\n{}", render(&doc));
    // Reserved target: error, document untouched.
    let before = render(&doc);
    let a = find(&doc, "A").unwrap();
    assert!(matches!(move_to_section(&mut doc, a, Some("Done")), Err(OpError::Reserved(_))));
    assert_eq!(render(&doc), before);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test feed::ops::tests::move_to`
Expected: compile error — `move_to_section` not found.

- [ ] **Step 3: Implement**

```rust
/// Move an item (state, body, and tokens intact) to the end of the named
/// section — created if missing — or, when `target` is None, to the end of
/// the first human section (the uncategorized region). The reserved check
/// runs before the item is removed so an error leaves the doc untouched.
pub fn move_to_section(
    doc: &mut Document,
    index: usize,
    target: Option<&str>,
) -> Result<(), OpError> {
    if let Some(name) = target {
        if is_reserved_section(name) {
            return Err(OpError::Reserved(name.to_string()));
        }
    }
    let item = remove(doc, index)
        .ok_or_else(|| OpError::NotFound(format!("item at index {index}")))?;
    let at = match target {
        Some(name) => ensure_section(doc, name)?,
        None => end_of_first_human_section(doc),
    };
    doc.nodes.insert(at, Node::Item(item));
    Ok(())
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test feed::ops::tests::move_to`
Expected: 3 passed. Then `cargo test` — all green.

- [ ] **Step 5: Commit**

```bash
git add src/feed/ops.rs
git commit -m "feat: move_to_section relocates items between categories"
```

---

### Task 6: Modal state — Category field, suggestions, focus order

**Files:**
- Modify: `src/tui/modal.rs` (`EditFocus` ~line 34, `EditModal` ~line 49, `create`/`edit`/`cycle_focus`/`sync_blocks` ~lines 63-126; tests in its `mod tests`)
- Modify: `src/tui/app.rs:294-301` (`OpenEdit`/`OpenCreate` call sites — same task so the build stays green)

- [ ] **Step 1: Write the failing tests** (in `src/tui/modal.rs` `mod tests`)

```rust
#[test]
fn create_focuses_category_first_and_cycles_through_it() {
    let mut m = EditModal::create(vec!["Work".into()]);
    assert_eq!(m.focus, EditFocus::Category);
    m.cycle_focus();
    assert_eq!(m.focus, EditFocus::Title);
    m.cycle_focus(); // Body
    m.cycle_focus(); // Save
    m.cycle_focus(); // Cancel
    m.cycle_focus(); // back to Category (create mode: no Delete)
    assert_eq!(m.focus, EditFocus::Category);
}

#[test]
fn filtered_matches_substring_case_insensitive() {
    let mut m = EditModal::create(vec!["Work".into(), "Chores".into(), "Homework".into()]);
    assert_eq!(m.filtered(), vec!["Work", "Chores", "Homework"]); // empty query = all
    m.category.insert_str("ork");
    assert_eq!(m.filtered(), vec!["Work", "Homework"]);
    m.set_category("Chores");
    assert_eq!(m.category_text(), "Chores");
}

#[test]
fn edit_prefills_category_from_section() {
    let item = Item {
        state: State::Open,
        title: "T".into(),
        agent: None,
        done_date: None,
        body: Vec::new(),
    };
    let key = ItemKey { title: "T".into(), state: State::Open };
    let m = EditModal::edit(key.clone(), &item, Some("Work".into()), vec!["Work".into()]);
    assert_eq!(m.category_text(), "Work");
    assert_eq!(m.original_category.as_deref(), Some("Work"));
    assert_eq!(m.focus, EditFocus::Category);
    let m = EditModal::edit(key, &item, None, vec![]);
    assert_eq!(m.category_text(), "");
    assert_eq!(m.original_category, None);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test tui::modal`
Expected: compile errors — `Category` variant, new constructor signatures.

- [ ] **Step 3: Implement**

`EditFocus` gains `Category` as the FIRST variant:

```rust
pub enum EditFocus {
    Category,
    Title,
    Body,
    Save,
    Cancel,
    /// Edit mode only — create mode has no item to delete.
    Delete,
}
```

`EditModal` (replace the struct and constructors; keep `title_text`/`body_lines`):

```rust
/// Visible dropdown rows are capped; the keyboard highlight is clamped to
/// the same cap so it can never point at an invisible row.
pub const MAX_DROPDOWN_ROWS: usize = 5;

pub struct EditModal {
    /// `Some(key)` = editing an existing item; `None` = creating.
    pub original: Option<ItemKey>,
    /// Edit mode: the section the item was in when the modal opened (None =
    /// uncategorized). Compared with `category_text()` on save to decide
    /// whether the item moves. Always None in create mode.
    pub original_category: Option<String>,
    pub category: TextArea<'static>,
    pub title: TextArea<'static>,
    pub body: TextArea<'static>,
    /// Category suggestions captured at open (`ops::section_names`).
    pub suggestions: Vec<String>,
    /// Keyboard highlight into `filtered()`; None = not in the list.
    pub dropdown: Option<usize>,
    pub focus: EditFocus,
}

impl EditModal {
    /// Sidebar-created items are always the human's. Category is the topmost
    /// field (spec 2026-09-16 §2): empty keeps the pre-category behavior
    /// (end of first human section), anything else names a section.
    pub fn create(suggestions: Vec<String>) -> Self {
        let mut m = EditModal {
            original: None,
            original_category: None,
            category: TextArea::default(),
            title: TextArea::default(),
            body: TextArea::default(),
            suggestions,
            dropdown: None,
            focus: EditFocus::Category,
        };
        m.sync_blocks();
        m
    }

    pub fn edit(
        key: ItemKey,
        item: &Item,
        section: Option<String>,
        suggestions: Vec<String>,
    ) -> Self {
        let mut m = EditModal {
            original: Some(key),
            category: TextArea::new(vec![section.clone().unwrap_or_default()]),
            original_category: section,
            title: TextArea::new(vec![item.title.clone()]),
            body: TextArea::new(item.body.clone()),
            suggestions,
            dropdown: None,
            focus: EditFocus::Category,
        };
        m.category.move_cursor(CursorMove::End);
        m.title.move_cursor(CursorMove::End);
        m.body.move_cursor(CursorMove::Bottom);
        m.body.move_cursor(CursorMove::End);
        m.sync_blocks();
        m
    }

    pub fn category_text(&self) -> String {
        self.category.lines().join(" ").trim().to_string()
    }

    /// Suggestions matching the field, case-insensitive substring.
    pub fn filtered(&self) -> Vec<String> {
        let q = self.category_text().to_lowercase();
        self.suggestions
            .iter()
            .filter(|s| s.to_lowercase().contains(&q))
            .cloned()
            .collect()
    }

    /// Replace the field content with an accepted suggestion.
    pub fn set_category(&mut self, name: &str) {
        self.category = TextArea::new(vec![name.to_string()]);
        self.category.move_cursor(CursorMove::End);
        self.sync_blocks();
    }

    /// Rows the dropdown occupies right now — the single source for both
    /// `view::draw_edit_modal` and `edit_step`'s hit-testing, so drawn and
    /// clickable rows can never drift apart.
    pub fn dropdown_rows(&self) -> u16 {
        if self.focus == EditFocus::Category {
            self.filtered().len().min(MAX_DROPDOWN_ROWS) as u16
        } else {
            0
        }
    }
    // ... existing methods ...
}
```

`cycle_focus` (Category first):

```rust
    pub fn cycle_focus(&mut self) {
        let is_edit = self.original.is_some();
        self.focus = match (self.focus, is_edit) {
            (EditFocus::Category, _) => EditFocus::Title,
            (EditFocus::Title, _) => EditFocus::Body,
            (EditFocus::Body, _) => EditFocus::Save,
            (EditFocus::Save, _) => EditFocus::Cancel,
            (EditFocus::Cancel, true) => EditFocus::Delete,
            (EditFocus::Cancel, false) => EditFocus::Category,
            (EditFocus::Delete, _) => EditFocus::Category,
        };
        self.sync_blocks();
    }
```

`sync_blocks` — add the category block alongside title/body:

```rust
        let c = if self.focus == EditFocus::Category {
            "Category*"
        } else {
            "Category"
        };
        self.category.set_block(Block::bordered().title(c));
        self.category.set_cursor_style(Style::default());
```

In `edit_step`'s trailing focus match, add (dropdown-reset on typing — the list refilters, so a stale highlight must not survive):

```rust
        EditFocus::Category => {
            m.category.input(ev);
            m.dropdown = None;
        }
```

Update `src/tui/app.rs` call sites (behavior unchanged beyond passing data):

```rust
            Action::OpenEdit(key) => {
                if let Some(i) = relocate(&self.doc, &key) {
                    if let Node::Item(it) = &self.doc.nodes[i] {
                        let section = ops::item_section(&self.doc, i);
                        let suggestions = ops::section_names(&self.doc);
                        self.set_modal(Modal::Edit(EditModal::edit(key, it, section, suggestions)));
                    }
                }
            }
            Action::OpenCreate => {
                let suggestions = ops::section_names(&self.doc);
                self.set_modal(Modal::Edit(EditModal::create(suggestions)))
            }
```

- [ ] **Step 4: Fix now-broken existing tests, run the suite**

Run: `cargo test`
Existing tests that open the modal and start typing assume Title focus (`modal.rs` round-2 tests ~line 535/725; `app.rs` `create_modal_save_creates_feed_file_on_first_run` ~line 947, `create_modal_adds_to_first_human_section` ~line 1003; any `view.rs` test constructing `EditModal::create()`). Fix pattern: after open, press Enter once (Category empty, dropdown closed → focus moves to Title in Task 7; until then use `Tab`) — for THIS task use `press(&mut app, KeyCode::Tab)` / `m.cycle_focus()` to reach Title, and update stale "no section picker" comments to reference the Category field (spec 2026-09-16). Constructor calls in tests become `EditModal::create(vec![])`.
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: category field state in the edit/create modal"
```

---

### Task 7: Modal geometry — category rect, dropdown rect, click targets

**Files:**
- Modify: `src/tui/modal.rs` (`EditLayout` ~line 150, `edit_layout` ~line 198, `EditClickTarget` ~line 232, `edit_click` ~line 248; tests)
- Modify: `src/tui/view.rs` call sites/tests referencing `edit_layout` (~lines 248, 686, 788)

- [ ] **Step 1: Write the failing tests** (in `src/tui/modal.rs` `mod tests`)

```rust
#[test]
fn layout_stacks_category_above_title_and_sizes_dropdown() {
    let l = edit_layout(TEST_AREA, false, 3);
    assert!(l.category.y < l.title.y && l.title.y < l.body.y);
    assert_eq!(l.dropdown.height, 3);
    assert_eq!(l.dropdown.y, l.category.y + l.category.height);
    let l0 = edit_layout(TEST_AREA, false, 0);
    assert_eq!(l0.dropdown.height, 0);
}

#[test]
fn clicks_hit_category_and_dropdown_rows() {
    let l = edit_layout(TEST_AREA, false, 2);
    assert_eq!(
        edit_click(&l, false, l.category.x + 1, l.category.y + 1),
        Some(EditClickTarget::Category)
    );
    // The dropdown overlays the title area — it must win the hit-test:
    assert_eq!(
        edit_click(&l, false, l.dropdown.x + 1, l.dropdown.y + 1),
        Some(EditClickTarget::Suggestion(1))
    );
    let l0 = edit_layout(TEST_AREA, false, 0);
    assert_eq!(
        edit_click(&l0, false, l0.title.x + 1, l0.title.y + 1),
        Some(EditClickTarget::Title)
    );
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test tui::modal`
Expected: compile errors — `edit_layout` arity, `category`/`dropdown` fields, `Category`/`Suggestion` variants.

- [ ] **Step 3: Implement**

`EditLayout` gains:

```rust
    pub category: Rect,
    /// Zero-height when the dropdown is closed. Overlays the title/body
    /// area; `edit_click` tests it first so overlap resolves to the list.
    pub dropdown: Rect,
```

`edit_layout` — new `dropdown_rows` parameter, category row added to the stack (the button clamping stays exactly as-is; see the crash-fix comment above it):

```rust
pub fn edit_layout(term_area: Rect, is_edit: bool, dropdown_rows: u16) -> EditLayout {
    let outer = centered(term_area, 90, 80);
    let inner = Block::bordered().padding(Padding::uniform(1)).inner(outer);
    let [category, title, body, buttons, hints] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    // Inside the category field's borders, clamped like the buttons so it
    // can never escape the drawn buffer.
    let dropdown = Rect::new(
        category.x.saturating_add(1),
        category.y.saturating_add(category.height),
        category.width.saturating_sub(2),
        dropdown_rows,
    )
    .intersection(term_area);
    // ... existing button-row code unchanged ...
    EditLayout { outer, category, dropdown, title, body, hints, save, cancel, delete }
}
```

`EditClickTarget` gains `Category` and `Suggestion(u16)`; `edit_click` tests dropdown first, then category, then the existing targets:

```rust
pub fn edit_click(layout: &EditLayout, is_edit: bool, x: u16, y: u16) -> Option<EditClickTarget> {
    if layout.dropdown.height > 0 && rect_contains(layout.dropdown, x, y) {
        return Some(EditClickTarget::Suggestion(y - layout.dropdown.y));
    }
    if rect_contains(layout.category, x, y) {
        return Some(EditClickTarget::Category);
    }
    // ... existing title/body/save/cancel/delete checks unchanged ...
}
```

Update every `edit_layout(area, is_edit)` caller to pass a rows argument: `edit_step`'s mouse arm uses `m.dropdown_rows()`; `view::draw_edit_modal` likewise (Task 8/9 wire these — for now pass `0` where the modal state isn't in scope); tests at `modal.rs` ~line 414+ and `view.rs` ~lines 686/788 pass `0`.

- [ ] **Step 4: Run the suite**

Run: `cargo test`
Expected: all green (including the narrow-width button-clamp tests, untouched).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: modal geometry for the category field and its dropdown"
```

---

### Task 8: Modal transitions — keys and clicks for the dropdown

**Files:**
- Modify: `src/tui/modal.rs` (`edit_step` ~line 340; tests)

- [ ] **Step 1: Write the failing tests**

```rust
fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn step_edit(m: EditModal, ev: Event) -> EditModal {
    match step(Modal::Edit(m), ev, TEST_AREA) {
        ModalStep::Continue(Modal::Edit(m)) => m,
        s => panic!("expected Continue(Edit), got another step"),
    }
}

#[test]
fn down_enters_dropdown_enter_accepts() {
    let m = EditModal::create(vec!["Work".into(), "Chores".into()]);
    let m = step_edit(m, key(KeyCode::Down));
    assert_eq!(m.dropdown, Some(0));
    let m = step_edit(m, key(KeyCode::Down));
    assert_eq!(m.dropdown, Some(1)); // clamped at the end, not wrapping
    let m = step_edit(m, key(KeyCode::Down));
    assert_eq!(m.dropdown, Some(1));
    let m = step_edit(m, key(KeyCode::Enter));
    assert_eq!(m.category_text(), "Chores");
    assert_eq!(m.dropdown, None);
    assert_eq!(m.focus, EditFocus::Category, "accept stays on the field");
}

#[test]
fn enter_with_dropdown_closed_advances_to_title() {
    let m = EditModal::create(vec!["Work".into()]);
    let m = step_edit(m, key(KeyCode::Enter));
    assert_eq!(m.focus, EditFocus::Title);
}

#[test]
fn esc_closes_dropdown_first_then_cancels() {
    let m = EditModal::create(vec!["Work".into()]);
    let m = step_edit(m, key(KeyCode::Down));
    assert_eq!(m.dropdown, Some(0));
    let m = step_edit(m, key(KeyCode::Esc)); // first Esc: dropdown only
    assert_eq!(m.dropdown, None);
    match step(Modal::Edit(m), key(KeyCode::Esc), TEST_AREA) {
        ModalStep::Continue(Modal::None) => {} // second Esc: modal cancels
        _ => panic!("second Esc must cancel the modal"),
    }
}

#[test]
fn typing_filters_and_resets_highlight_up_leaves_list() {
    let m = EditModal::create(vec!["Work".into(), "Homework".into(), "Chores".into()]);
    let m = step_edit(m, key(KeyCode::Down));
    let m = step_edit(m, key(KeyCode::Char('w')));
    assert_eq!(m.dropdown, None, "typing resets the highlight");
    assert_eq!(m.filtered(), vec!["Work", "Homework"]);
    let m = step_edit(m, key(KeyCode::Down));
    let m = step_edit(m, key(KeyCode::Up));
    assert_eq!(m.dropdown, None, "Up at the top leaves the list");
}

#[test]
fn tab_closes_dropdown_and_moves_focus() {
    let m = EditModal::create(vec!["Work".into()]);
    let m = step_edit(m, key(KeyCode::Down));
    let m = step_edit(m, key(KeyCode::Tab));
    assert_eq!(m.dropdown, None);
    assert_eq!(m.focus, EditFocus::Title);
}

#[test]
fn click_on_suggestion_accepts_it() {
    let m = EditModal::create(vec!["Work".into(), "Chores".into()]);
    let m = step_edit(m, key(KeyCode::Down)); // dropdown open (2 rows)
    let l = edit_layout(TEST_AREA, false, m.dropdown_rows());
    let ev = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: l.dropdown.x + 1,
        row: l.dropdown.y + 1,
        modifiers: KeyModifiers::NONE,
    });
    let m = step_edit(m, ev);
    assert_eq!(m.category_text(), "Chores");
    assert_eq!(m.dropdown, None);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test tui::modal`
Expected: FAIL — Down currently reaches the textarea (moves cursor), Esc cancels immediately, Enter on Category not handled.

- [ ] **Step 3: Implement in `edit_step`**

Replace the Esc and Tab arms and add Category arms BEFORE the existing `Enter`/fallthrough handling:

```rust
            (KeyCode::Esc, _) => {
                if m.focus == EditFocus::Category && m.dropdown.is_some() {
                    m.dropdown = None;
                    return ModalStep::Continue(Modal::Edit(m));
                }
                return ModalStep::Continue(Modal::None);
            }
            (KeyCode::Tab, _) => {
                m.dropdown = None;
                m.cycle_focus();
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Down, _) if m.focus == EditFocus::Category => {
                let n = m.filtered().len().min(MAX_DROPDOWN_ROWS);
                if n > 0 {
                    m.dropdown = Some(m.dropdown.map_or(0, |i| (i + 1).min(n - 1)));
                }
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Up, _) if m.focus == EditFocus::Category => {
                m.dropdown = match m.dropdown {
                    Some(i) if i > 0 => Some(i - 1),
                    _ => None,
                };
                return ModalStep::Continue(Modal::Edit(m));
            }
            (KeyCode::Enter, _) if m.focus == EditFocus::Category => {
                if let Some(i) = m.dropdown {
                    if let Some(name) = m.filtered().get(i).cloned() {
                        m.set_category(&name);
                    }
                    m.dropdown = None;
                } else {
                    m.focus = EditFocus::Title;
                    m.sync_blocks();
                }
                return ModalStep::Continue(Modal::Edit(m));
            }
```

In the mouse arm, derive the layout from live state and handle the new targets:

```rust
            let layout = edit_layout(area, is_edit, m.dropdown_rows());
            return match edit_click(&layout, is_edit, mev.column, mev.row) {
                Some(EditClickTarget::Suggestion(row)) => {
                    if let Some(name) = m.filtered().get(row as usize).cloned() {
                        m.set_category(&name);
                    }
                    m.dropdown = None;
                    ModalStep::Continue(Modal::Edit(m))
                }
                Some(EditClickTarget::Category) => {
                    m.focus = EditFocus::Category;
                    m.dropdown = None;
                    m.sync_blocks();
                    ModalStep::Continue(Modal::Edit(m))
                }
                // ... existing Title/Body/Save/Cancel/Delete arms unchanged ...
            };
```

- [ ] **Step 4: Run the suite; simplify Task 6's stopgaps**

Run: `cargo test`
Expected: all green. Tests that used `Tab` to skip an empty Category (Task 6 step 4) can now press Enter instead where that reads better — optional cleanup.

- [ ] **Step 5: Commit**

```bash
git add src/tui/modal.rs src/tui/app.rs
git commit -m "feat: category dropdown keyboard and mouse transitions"
```

---

### Task 9: Rendering — category field, dropdown list, cursor

**Files:**
- Modify: `src/tui/view.rs` (`draw_edit_modal` ~line 246; imports if `Modifier` missing)
- Modify: `src/tui/modal.rs` (`edit_layout` short-height guard; narrow-width sweep test)

Review follow-ups folded in (from Task 8's code review):
- Clear `m.dropdown = None` in the Title and Body click arms (a stale highlight
  currently survives a focus-moving click; symmetric with Tab, which clears).
- Extract `fn accept_suggestion(&mut self, i: usize)` on `EditModal` (lookup,
  `set_category`, `dropdown = None`) and use it from both the Enter arm and the
  Suggestion click arm.
- `click_on_suggestion_accepts_it` should use the existing `click(x, y)` test helper
  instead of building a `MouseEvent` inline.

Review follow-ups folded in (from Task 7's code review):
- **Short-height guard**: at inner heights ≲8 the constraint solver starves Title
  (title_h drops to 0-2 → invisible text in a bordered TextArea). In `edit_layout`,
  collapse the category row to `Constraint::Length(0)` when `inner.height < 9` so
  Title keeps priority on very short panes; add a test asserting title.height >= 3
  at a 30x12 terminal with the category row collapsed.
- **Sweep extension**: the narrow-width crash-regression sweep in modal.rs tests
  only varies width at rows=0 — extend it to `dropdown_rows in [0, 5]` (and a couple
  of short heights) asserting the `dropdown` rect stays inside the terminal area.

- [ ] **Step 1: Write the failing test** (in `src/tui/view.rs` `mod tests`, using the existing `TestBackend` pattern — mirror a nearby modal-drawing test's setup)

```rust
#[test]
fn edit_modal_draws_category_field_and_dropdown() {
    let mut m = modal::EditModal::create(vec!["Work".into(), "Chores".into()]);
    // Open the dropdown so both the field and the list render:
    m.dropdown = Some(0);
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|f| draw_edit_modal(f, &m))
        .unwrap();
    let text = format!("{:?}", terminal.backend().buffer());
    assert!(text.contains("Category"), "category field must render");
    assert!(text.contains("Work") && text.contains("Chores"), "suggestions must render");
}
```

(If nearby view tests read the buffer differently — e.g. a helper that joins buffer cells into strings — use that helper instead of `format!("{:?}")`.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test tui::view::tests::edit_modal_draws_category`
Expected: FAIL — "Category" not rendered.

- [ ] **Step 3: Implement in `draw_edit_modal`**

```rust
fn draw_edit_modal(f: &mut Frame, m: &modal::EditModal) {
    let is_edit = m.original.is_some();
    let layout = modal::edit_layout(f.area(), is_edit, m.dropdown_rows());

    // ... existing Clear + outer block unchanged ...

    f.render_widget(&m.category, layout.category);
    f.render_widget(&m.title, layout.title);
    f.render_widget(&m.body, layout.body);

    // ... existing buttons + hint unchanged ...

    // Dropdown last so it overlays the title/body area.
    if layout.dropdown.height > 0 {
        f.render_widget(Clear, layout.dropdown);
        let rows: Vec<Line> = m
            .filtered()
            .iter()
            .take(layout.dropdown.height as usize)
            .enumerate()
            .map(|(i, s)| {
                let style = if m.dropdown == Some(i) {
                    theme::normal_text().add_modifier(Modifier::REVERSED)
                } else {
                    theme::normal_text()
                };
                Line::styled(s.clone(), style)
            })
            .collect();
        f.render_widget(
            Paragraph::new(rows).style(theme::modal_panel_style()),
            layout.dropdown,
        );
    }

    match m.focus {
        EditFocus::Category => {
            f.set_cursor_position(cursor_screen_pos(layout.category, m.category.cursor()))
        }
        EditFocus::Title => {
            f.set_cursor_position(cursor_screen_pos(layout.title, m.title.cursor()))
        }
        EditFocus::Body => f.set_cursor_position(cursor_screen_pos(layout.body, m.body.cursor())),
        _ => {}
    }
}
```

Add `Modifier` to the ratatui style imports if not present (`use ratatui::style::Modifier;`).

- [ ] **Step 4: Run the suite**

Run: `cargo test`
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add src/tui/view.rs
git commit -m "feat: render the category field and suggestion dropdown"
```

---

### Task 10: Save wiring — create in category, move on edit

**Files:**
- Modify: `src/tui/app.rs` (`save_modal` ~line 404; tests in its `mod tests`, using the existing `app_on_disk`/`press`/`type_str`/`feed_text` helpers)
- Modify: `src/tui/modal.rs` (Task-9 review riders)

Review follow-ups folded in (from Task 9's code review):
- **Resize hole**: the Enter arm accepts `m.dropdown = Some(i)` without re-checking
  visibility — after a terminal shrink, Enter can still accept a clipped-off (unseen)
  suggestion. Extract a shared `visible_rows(&m, area)` helper (the Down arm's clamp)
  and apply it in the Enter arm too (ignore/clamp a highlight past the drawn rows).
- Simplify/comment the Down clamp (`visible` already subsumes the other two terms).
- Rename sweep test `edit_layout_buttons_never_escape_the_terminal_at_any_width` →
  `edit_layout_rects_never_escape_the_terminal` (it now sweeps heights + dropdown).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn create_with_category_lands_in_that_section_creating_it() {
    let (mut app, _fake, _dir) = app_on_disk("# Feed\n\n- [ ] A\n\n## Agent\n");
    app.apply(Action::OpenCreate);
    type_str(&mut app, "Work"); // Category field is focused first
    press(&mut app, KeyCode::Enter); // → Title
    type_str(&mut app, "New item");
    press_ctrl(&mut app, 's');
    assert!(!app.modal_active());
    let out = feed_text(&app);
    assert!(out.contains("## Work\n\n- [ ] New item\n"), "got:\n{out}");
    assert!(out.find("## Work").unwrap() < out.find("## Agent").unwrap(), "got:\n{out}");
}

#[test]
fn create_with_existing_category_appends_case_insensitive() {
    let (mut app, _fake, _dir) = app_on_disk("# Feed\n\n## Work\n\n- [ ] W\n");
    app.apply(Action::OpenCreate);
    type_str(&mut app, "work");
    press(&mut app, KeyCode::Enter);
    type_str(&mut app, "New item");
    press_ctrl(&mut app, 's');
    let out = feed_text(&app);
    assert!(out.contains("- [ ] W\n- [ ] New item\n"), "got:\n{out}");
    assert!(!out.contains("## work"), "must not duplicate the section:\n{out}");
}

#[test]
fn edit_changing_category_moves_item_with_body() {
    let (mut app, _fake, _dir) =
        app_on_disk("# Feed\n\n## Work\n\n- [~] T @agent(claude:abc)\n  ctx\n\n## Chores\n\n- [ ] C\n");
    app.apply(Action::OpenEdit(ItemKey { title: "T".into(), state: State::InProgress }));
    let Modal::Edit(m) = &app.modal else { panic!("expected edit modal") };
    assert_eq!(m.category_text(), "Work"); // prefilled
    // Clear "Work", type "Chores":
    for _ in 0..4 {
        press(&mut app, KeyCode::Backspace);
    }
    type_str(&mut app, "Chores");
    press_ctrl(&mut app, 's');
    let out = feed_text(&app);
    assert!(out.contains("- [ ] C\n- [~] T @agent(claude:abc)\n  ctx\n"), "got:\n{out}");
    assert!(out.contains("## Work"), "emptied heading kept:\n{out}");
}

#[test]
fn edit_keeping_category_does_not_move_and_stays_byte_stable() {
    let text = "# Feed\n\n## Work\n\n- [ ] First\n- [ ] Second\n";
    let (mut app, _fake, _dir) = app_on_disk(text);
    app.apply(Action::OpenEdit(ItemKey { title: "First".into(), state: State::Open }));
    press_ctrl(&mut app, 's'); // change nothing
    assert_eq!(feed_text(&app), text, "untouched save must be byte-stable");
}

#[test]
fn reserved_category_rejected_with_status() {
    let (mut app, _fake, _dir) = app_on_disk("# Feed\n\n- [ ] A\n");
    app.apply(Action::OpenCreate);
    type_str(&mut app, "Agent");
    press(&mut app, KeyCode::Enter);
    type_str(&mut app, "Sneaky");
    press_ctrl(&mut app, 's');
    assert!(app.modal_active(), "save must be refused");
    assert_eq!(app.status_msg.as_deref(), Some("\"Agent\" is a reserved section"));
}

#[test]
fn empty_category_keeps_first_human_section_behavior() {
    let (mut app, _fake, _dir) = app_on_disk("# Feed\n\n- [ ] A\n\n## Later\n\n- [ ] L1\n");
    app.apply(Action::OpenCreate);
    press(&mut app, KeyCode::Enter); // empty Category → Title
    type_str(&mut app, "New item");
    press_ctrl(&mut app, 's');
    assert!(feed_text(&app).contains("- [ ] A\n- [ ] New item\n"));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test tui::app`
Expected: FAIL — category text is ignored by `save_modal` (items land in the first human section; no reserved check; edit never moves).

- [ ] **Step 3: Implement `save_modal`**

```rust
    fn save_modal(&mut self, m: EditModal) -> Modal {
        let title = m.title_text();
        if title.is_empty() {
            self.status_msg = Some("title required".into());
            return Modal::Edit(m);
        }
        let category = m.category_text();
        if !category.is_empty() && ops::is_reserved_section(&category) {
            self.status_msg = Some(format!("\"{category}\" is a reserved section"));
            return Modal::Edit(m);
        }
        let body = m.body_lines();
        match &m.original {
            Some(key) => {
                let key = key.clone();
                let target: Option<String> = (!category.is_empty()).then(|| category.clone());
                // Case-insensitive: renaming "work" to "Work" is not a move.
                let moved = match (&m.original_category, &target) {
                    (Some(a), Some(b)) => !a.eq_ignore_ascii_case(b),
                    (None, None) => false,
                    _ => true,
                };
                self.with_feed(move |doc| match relocate(doc, &key) {
                    Some(i) => {
                        ops::edit(doc, i, &title, &body);
                        if moved {
                            ops::move_to_section(doc, i, target.as_deref())?;
                        }
                        Ok(Outcome::Changed(None))
                    }
                    None => Ok(Outcome::Unchanged(Some(
                        "item changed on disk — edit dropped".into(),
                    ))),
                });
            }
            None => {
                // Sidebar-created items are always the human's. An empty
                // Category keeps the pre-category behavior (end of the first
                // human section); a named one targets that section, creating
                // it if needed (spec 2026-09-16 §2).
                self.with_feed(move |doc| {
                    if category.is_empty() {
                        ops::add(doc, &title, &body, Zone::Human);
                    } else {
                        ops::add_in_section(doc, &title, &body, &category)?;
                    }
                    Ok(Outcome::Changed(None))
                });
            }
        }
        Modal::None
    }
```

(`?` on `OpError` inside the `with_feed` closure converts via anyhow, landing in the existing `Err(e) => status_msg` arm without saving — belt and braces behind the reserved pre-check.)

- [ ] **Step 4: Run the full suite, fmt, clippy**

Run: `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: all green, no lint errors.

- [ ] **Step 5: Commit**

```bash
git add src/tui/app.rs
git commit -m "feat: create and move items by category from the modal"
```

---

### Task 11: Docs — spec cross-link + SPEC.md note (+ one Task-10 review rider)

Rider from Task 10's code review: add one end-to-end test in `src/tui/app.rs` —
open edit on a `## Work` item, backspace the Category field empty, Ctrl-S, assert
the item lands at the end of the uncategorized region (the `Some→None` arm of the
moved-matrix has ops-level coverage but no modal-path witness).

**Files:**
- Modify: `docs/SPEC.md` (§3 sidebar interactions, ~line 69)
- Verify: `docs/superpowers/specs/2026-09-16-category-field-design.md` reserved-names wording matches the implementation (Agent, Done, **and Feed**)

- [ ] **Step 1: Update SPEC.md §3** — replace the `+ row / a` bullet:

```markdown
- **`+` row / `a`** opens the same modal empty — Category (topmost, with a suggestion
  dropdown over existing and archived section names; free text creates a new `##`
  section), title, optional body. An empty Category lands the item at the end of the
  first human section. Editing an item's Category moves it to the end of the target
  section. Delete lives in the modal, with confirm. See
  `docs/superpowers/specs/2026-09-16-category-field-design.md`.
```

- [ ] **Step 2: Amend the design spec** — in §1/§2, note that "Feed" is also reserved (it is the archive's mirror name for uncategorized items), matching `ops::RESERVED_SECTIONS`.

- [ ] **Step 3: Full verification**

Run: `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add docs/
git commit -m "docs: category field in SPEC and design spec reserved-name note"
```

---

## Self-Review Notes (already applied)

- Spec §2 dropdown semantics (Down/Up/Enter/Esc layering, Enter-closed → Title) → Task 8 tests cover each.
- Spec §3 ops (`ensure_section` placement, `move_to_section`) → Tasks 2, 5.
- Spec §4 concurrency (drop-on-miss) → existing `relocate` path reused in Task 10; `edit_keeping_category_does_not_move_and_stays_byte_stable` covers the round-trip requirement (§8).
- Spec §5 (empty sections kept) → Task 5 test.
- Spec §6 (view unchanged) → no view list changes; only the modal renderer (Task 9).
- Reserved set includes "Feed" (implementation detail beyond spec's Agent/Done) → Task 11 amends the spec.
- CLI `feedr add --section` behavior change (creates sections; `--section Agent` now errors) → intended, noted in Task 3.
