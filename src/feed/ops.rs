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
    #[error("\"{0}\" is ambiguous: {list}", list = .1.join(", "))]
    Ambiguous(String, Vec<String>),
    #[error("only the human closes human-created items (use review, or --as-human)")]
    NotAuthorised,
    #[error("\"{0}\" is a reserved section")]
    Reserved(String),
    #[error("section name required")]
    EmptyName,
}

pub fn zone_of(doc: &Document, index: usize) -> Zone {
    let mut zone = Zone::Human;
    for node in &doc.nodes[..=index] {
        if let Node::Heading { level, text } = node {
            match level {
                1 if text.eq_ignore_ascii_case("Done") => zone = Zone::Archive,
                1 => zone = Zone::Human,
                2 if zone != Zone::Archive => {
                    zone = if text.eq_ignore_ascii_case("Agent") {
                        Zone::Agent
                    } else {
                        Zone::Human
                    };
                }
                _ => {}
            }
        }
    }
    zone
}

pub fn find(doc: &Document, query: &str) -> Result<usize, OpError> {
    let q = query.to_lowercase();
    let matches: Vec<usize> = doc
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| match n {
            Node::Item(it)
                if zone_of(doc, i) != Zone::Archive && it.title.to_lowercase().contains(&q) =>
            {
                Some(i)
            }
            _ => None,
        })
        .collect();
    match matches.len() {
        0 => Err(OpError::NotFound(query.to_string())),
        1 => Ok(matches[0]),
        _ => Err(OpError::Ambiguous(
            query.to_string(),
            matches
                .iter()
                .map(|&i| match &doc.nodes[i] {
                    Node::Item(it) => it.title.clone(),
                    _ => unreachable!(),
                })
                .collect(),
        )),
    }
}

/// Claiming is last-writer-wins by design (spec: "one @agent token per item,
/// latest claim wins") — re-claiming an item reopens it and replaces the tag.
pub fn claim(doc: &mut Document, index: usize, agent: AgentRef) {
    debug_assert!(matches!(doc.nodes[index], Node::Item(_)));
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
        body: trim_blank_edges(body),
    });
    let insert_at = match zone {
        Zone::Human => end_of_first_human_section(doc),
        Zone::Agent | Zone::Archive => match agent_section_end(doc) {
            Some(i) => i,
            None => {
                // Create "## Agent" just before "# Done" (or at EOF).
                let at = doc.nodes.iter().position(
                    |n| matches!(n, Node::Heading { level: 1, text } if text.eq_ignore_ascii_case("Done")),
                );
                let at = at.unwrap_or(doc.nodes.len());
                doc.nodes.insert(at, Node::Raw(String::new()));
                doc.nodes.insert(
                    at,
                    Node::Heading {
                        level: 2,
                        text: "Agent".into(),
                    },
                );
                doc.nodes.insert(at, Node::Raw(String::new()));
                at + 3
            }
        },
    };
    doc.nodes.insert(insert_at, item);
}

/// Trims leading/trailing empty lines from a provided body while keeping
/// interior blanks (e.g. blank lines inside a fenced code block).
fn trim_blank_edges(lines: &[String]) -> Vec<String> {
    let start = lines
        .iter()
        .position(|l| !l.is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(start, |i| i + 1);
    lines[start..end].to_vec()
}

/// Insertion index for a new human item: just past the last item of the first
/// non-empty human region, or — when no human items exist yet — directly
/// before the boundary heading (`## Agent` / `# Done`), stepping back over a
/// single preceding blank line; end of document when there is no boundary.
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

/// Insertion index for the true uncategorized region: just past the last
/// item before the first `##` heading (or `# Done`), stepping back over a
/// single preceding blank when the region holds no items. Unlike
/// `end_of_first_human_section` (the legacy create path), a named section
/// is ALWAYS a boundary here — clearing an item's category must never land
/// it inside another category.
fn end_of_uncategorized_region(doc: &Document) -> usize {
    let mut last_item_end = 0usize;
    let mut boundary: Option<usize> = None;
    for (i, n) in doc.nodes.iter().enumerate() {
        match n {
            Node::Heading { level: 2, .. } => {
                boundary = Some(i);
                break;
            }
            Node::Heading { level: 1, text } if text.eq_ignore_ascii_case("Done") => {
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
    named_section_end(doc, "Agent")
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

/// Section names that can never be categories: "Agent" is the agents' zone,
/// "Done" the archive, "Feed" the archive's mirror name for uncategorized
/// items (spec 2026-09-16 §1).
/// Not yet called outside tests — wired up by the category feature's later
/// tasks (category validation, modal suggestions).
#[allow(dead_code)]
const RESERVED_SECTIONS: [&str; 3] = ["agent", "done", "feed"];

#[allow(dead_code)]
pub fn is_reserved_section(name: &str) -> bool {
    RESERVED_SECTIONS
        .iter()
        .any(|r| name.eq_ignore_ascii_case(r))
}

/// Category suggestions for the modal: every `##` heading in the file —
/// active sections plus the `# Done` archive's mirrored names ("categories
/// used in the past") — first-seen casing, case-insensitively deduplicated,
/// reserved names excluded.
/// Not yet called outside tests — wired up by a later task.
#[allow(dead_code)]
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

/// Trimmed, validated section name — the single gate both `ensure_section`
/// and `move_to_section` use, so their checks can never drift apart (the
/// no-drop invariant in `move_to_section` depends on this being the SAME
/// validation `ensure_section` applies).
fn validate_section_name(name: &str) -> Result<&str, OpError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(OpError::EmptyName);
    }
    if is_reserved_section(name) {
        return Err(OpError::Reserved(name.to_string()));
    }
    Ok(name)
}

/// Find the active `## name` section (case-insensitive; archive subsections
/// never match) or create it at the end of the human zone — before
/// `## Agent` if present, else before `# Done`, else at EOF. Returns the
/// insertion index for a new item at the section's end.
pub fn ensure_section(doc: &mut Document, name: &str) -> Result<usize, OpError> {
    let name = validate_section_name(name)?;
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

/// Move an item (state, body, and tokens intact) to the end of the named
/// section — created if missing — or, when `target` is None, to the end of
/// the uncategorized region (never into another category). The
/// reserved/empty checks run before the item is removed so an error leaves
/// the doc untouched.
#[allow(dead_code)]
pub fn move_to_section(
    doc: &mut Document,
    index: usize,
    target: Option<&str>,
) -> Result<(), OpError> {
    if let Some(name) = target {
        validate_section_name(name)?;
    }
    let item =
        remove(doc, index).ok_or_else(|| OpError::NotFound(format!("item at index {index}")))?;
    let at = match target {
        Some(name) => ensure_section(doc, name)?,
        None => end_of_uncategorized_region(doc),
    };
    doc.nodes.insert(at, Node::Item(item));
    Ok(())
}

pub fn set_state(
    doc: &mut Document,
    index: usize,
    state: State,
    by: Authority,
) -> Result<(), OpError> {
    debug_assert!(matches!(doc.nodes[index], Node::Item(_)));
    if state == State::Done && by == Authority::Agent && zone_of(doc, index) == Zone::Human {
        return Err(OpError::NotAuthorised);
    }
    if let Node::Item(it) = &mut doc.nodes[index] {
        it.state = state;
    }
    Ok(())
}

/// The `##` section an item sits under, or None when it's uncategorized
/// (directly under a `#` heading). Prefills the modal's Category field —
/// unlike the private `section_name` (archive mirroring), this does not
/// fall back to the level-1 heading's name.
/// Not yet called outside tests — wired up in a later task.
#[allow(dead_code)]
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

/// Section name an item sits under, for mirroring in the Done archive.
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

/// Manual archive/cleanup pass: active `[x]` human items are moved under
/// `# Done` into a mirrored `##` section and stamped `@done(date)`; active
/// `[x]` agent-zone items are deleted outright; everything else is left
/// untouched. Returns the number of items swept (archived + deleted).
pub fn sweep(doc: &mut Document, today: &str) -> usize {
    let done_items: Vec<usize> = doc
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| match n {
            Node::Item(it) if it.state == State::Done && zone_of(doc, i) != Zone::Archive => {
                Some(i)
            }
            _ => None,
        })
        .collect();

    let swept = done_items.len();

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
        }
    }
    archived.reverse();

    if archived.is_empty() {
        return swept;
    }

    let done_at = doc.nodes.iter().position(
        |n| matches!(n, Node::Heading { level: 1, text } if text.eq_ignore_ascii_case("Done")),
    );
    let done_at = done_at.unwrap_or_else(|| {
        doc.nodes.push(Node::Raw(String::new()));
        doc.nodes.push(Node::Heading {
            level: 1,
            text: "Done".into(),
        });
        doc.nodes.len() - 1
    });

    for (section, item) in archived {
        let insert_at = match archive_insertion_point(doc, done_at, &section) {
            Some(at) => at,
            None => {
                // Create the missing mirrored section, but stay inside the
                // Done region — insert before the next top-level (`#`)
                // heading rather than at EOF, so later `# Notes`-style
                // sections aren't mistaken for part of the archive.
                let end = done_region_end(doc, done_at);
                doc.nodes.insert(end, Node::Raw(String::new()));
                doc.nodes.insert(
                    end + 1,
                    Node::Heading {
                        level: 2,
                        text: section,
                    },
                );
                doc.nodes.insert(end + 2, Node::Raw(String::new()));
                end + 3
            }
        };
        doc.nodes.insert(insert_at, Node::Item(item));
    }

    swept
}

/// End of the `# Done` region: the index of the next level-1 heading after
/// `done_at`, or the end of the document when there is none.
fn done_region_end(doc: &Document, done_at: usize) -> usize {
    doc.nodes
        .iter()
        .enumerate()
        .skip(done_at + 1)
        .find(|(_, n)| matches!(n, Node::Heading { level: 1, .. }))
        .map(|(i, _)| i)
        .unwrap_or(doc.nodes.len())
}

/// Index at which to insert a newly-archived item under the mirrored `##
/// <section>` heading inside `# Done` (whose own heading sits at `done_at`).
/// Stops at the next level-1 heading so a later `# Heading` reusing the same
/// `##` name is never mistaken for the archive section. Returns `None` when
/// no such subsection exists yet within `# Done`.
fn archive_insertion_point(doc: &Document, done_at: usize, section: &str) -> Option<usize> {
    let mut i = done_at + 1;
    while i < doc.nodes.len() {
        match &doc.nodes[i] {
            Node::Heading { level: 1, .. } => break,
            Node::Heading { level: 2, text } if text.eq_ignore_ascii_case(section) => {
                let mut j = i + 1;
                while j < doc.nodes.len() && !matches!(doc.nodes[j], Node::Heading { .. }) {
                    j += 1;
                }
                // Don't glue the new item after trailing blank lines; insert
                // right after the last real (non-blank) line of the section.
                while j > i + 1 && matches!(&doc.nodes[j - 1], Node::Raw(s) if s.is_empty()) {
                    j -= 1;
                }
                return Some(j);
            }
            _ => i += 1,
        }
    }
    None
}

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
        let idx: Vec<(usize, Zone)> = doc
            .nodes
            .iter()
            .enumerate()
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

    fn human_add_zone(text: &str) -> Zone {
        let mut doc = parse(text);
        add(&mut doc, "Human task", &[], Zone::Human);
        let i = find(&doc, "Human task").unwrap();
        zone_of(&doc, i)
    }

    #[test]
    fn add_mine_with_empty_human_region_stays_human() {
        assert_eq!(
            human_add_zone("# Feed\n\n## Agent\n\n- [ ] agent task\n"),
            Zone::Human
        );
        assert_eq!(
            human_add_zone("# Feed\n\n# Done\n\n## Feed\n\n- [x] Old thing @done(2026-09-01)\n"),
            Zone::Human
        );
        assert_eq!(human_add_zone("# Feed\n\n# Done\n"), Zone::Human);
    }

    #[test]
    fn add_keeps_single_interior_blank_body_line() {
        let mut doc = parse("- [ ] A\n");
        add(
            &mut doc,
            "B",
            &["one".into(), "".into(), "two".into()],
            Zone::Human,
        );
        let i = find(&doc, "B").unwrap();
        match &doc.nodes[i] {
            Node::Item(it) => assert_eq!(it.body, vec!["one", "", "two"]),
            n => panic!("expected item, got {n:?}"),
        }
    }

    #[test]
    fn add_keeps_interior_blank_body_lines() {
        let mut doc = parse("- [ ] A\n");
        add(
            &mut doc,
            "B",
            &["".into(), "one".into(), "".into(), "two".into(), "".into()],
            Zone::Human,
        );
        let i = find(&doc, "B").unwrap();
        match &doc.nodes[i] {
            Node::Item(it) => assert_eq!(it.body, vec!["one", "", "two"]),
            n => panic!("expected item, got {n:?}"),
        }
    }

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

    #[test]
    fn sweep_returns_count_of_swept_items() {
        // 2 human [x] archived + 1 agent-zone [x] deleted = 3 total swept.
        let text = "\
# Feed

- [x] Shipped thing
- [x] Another shipped thing
- [ ] Still open

## Agent

- [x] Agent chore

# Done

## Feed

- [x] Older thing @done(2026-09-01)
";
        let mut doc = parse(text);
        let n = sweep(&mut doc, "2026-09-11");
        assert_eq!(n, 3);
        // No active [x] items left → nothing to sweep, count is 0.
        let n2 = sweep(&mut doc, "2026-09-12");
        assert_eq!(n2, 0);
    }

    #[test]
    fn sweep_creates_done_when_absent() {
        let mut doc = parse("# Feed\n\n- [x] Ship it\n- [ ] Keep\n");
        sweep(&mut doc, "2026-09-11");
        let out = render(&doc);
        assert!(out.contains("# Done"));
        assert!(out.contains("- [x] Ship it @done(2026-09-11)"));
        assert!(out.find("Ship it").unwrap() > out.find("# Done").unwrap());
    }

    #[test]
    fn sweep_creates_missing_mirrored_section() {
        let mut doc = parse(
            "\
# Feed

## Work

- [x] W done

# Done

## Feed

- [x] Old @done(2026-09-01)
",
        );
        sweep(&mut doc, "2026-09-11");
        let out = render(&doc);
        let done = out.find("# Done").unwrap();
        let work_hdr = out.rfind("## Work").unwrap();
        assert!(
            work_hdr > done,
            "mirrored ## Work must be created inside Done"
        );
        assert!(out.contains("- [x] W done @done(2026-09-11)"));
    }

    #[test]
    fn sweep_is_idempotent_after_round_trip() {
        let text = "\
# Feed

- [x] A

## Work

- [x] B

# Done

## Feed

- [x] Old @done(2026-09-01)

## Other

- [x] Elsewhere @done(2026-08-01)
";
        let mut doc = parse(text);
        sweep(&mut doc, "2026-09-11");
        let once = render(&doc);
        let mut doc2 = parse(&once);
        sweep(&mut doc2, "2026-09-12");
        assert_eq!(render(&doc2), once);
    }

    #[test]
    fn sweep_insertion_keeps_blank_before_next_section() {
        let text = "\
# Feed

- [x] New thing

# Done

## Feed

- [x] Old @done(2026-09-01)

## Other

- [x] Elsewhere @done(2026-08-01)
";
        let mut doc = parse(text);
        sweep(&mut doc, "2026-09-11");
        let out = render(&doc);
        assert!(
            out.contains("- [x] Old @done(2026-09-01)\n- [x] New thing @done(2026-09-11)\n\n## Other"),
            "item must append directly after the last archive item, blank line preserved before ## Other; got:\n{out}"
        );
    }

    #[test]
    fn sweep_stays_inside_done_region_and_is_idempotent() {
        let text = "\
# Feed

- [x] Ship it

# Done

## Other

- [x] Old @done(2026-09-01)

# Notes

Some prose.
";
        let mut doc = parse(text);
        sweep(&mut doc, "2026-09-11");
        let once = render(&doc);
        let ship = once.find("Ship it").unwrap();
        assert!(
            ship > once.find("# Done").unwrap() && ship < once.find("# Notes").unwrap(),
            "archived item must sit inside the Done region:\n{once}"
        );
        let mut doc2 = parse(&once);
        sweep(&mut doc2, "2026-09-12");
        assert_eq!(render(&doc2), once, "second sweep must be a no-op");
    }

    #[test]
    fn edit_replaces_title_and_body_keeping_tokens() {
        let mut doc = parse("- [~] Old title @agent(claude:abc)\n  old body\n");
        let i = find(&doc, "old title").unwrap();
        edit(
            &mut doc,
            i,
            "New title",
            &["".into(), "new body".into(), "".into()],
        );
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
        let mut doc2 = parse("# Feed\n\n- [ ] A\n\n## Later\n\n- [ ] L1\n\n## Agent\n\n- [ ] G\n");
        add_in_section(&mut doc2, "L2", &["ctx".into()], "Later").unwrap();
        let out = render(&doc2);
        assert!(out.contains("- [ ] L1\n- [ ] L2\n  ctx\n"), "got:\n{out}");
        // A missing section is created (before ## Agent), not an error:
        add_in_section(&mut doc2, "X", &[], "Fresh").unwrap();
        let out = render(&doc2);
        assert!(
            out.find("## Fresh").unwrap() < out.find("## Agent").unwrap(),
            "got:\n{out}"
        );
        assert!(out.contains("## Fresh\n\n- [ ] X\n"), "got:\n{out}");
        // Reserved names error ("Feed" exists only under # Done in SAMPLE):
        let mut doc = parse(SAMPLE);
        assert!(matches!(
            add_in_section(&mut doc, "X", &[], "Feed"),
            Err(OpError::Reserved(_))
        ));
    }

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

    #[test]
    fn ensure_section_finds_existing_case_insensitive() {
        let mut doc = parse("# Feed\n\n## Work\n\n- [ ] W\n\n## Agent\n");
        let padded = ensure_section(&mut doc, " work ").unwrap();
        assert_eq!(padded, ensure_section(&mut doc, "work").unwrap());
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
        assert!(
            render(&doc).contains("- [ ] W\n- [ ] New\n"),
            "got:\n{}",
            render(&doc)
        );
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
        let mut doc =
            parse("# Feed\n\n- [ ] A\n\n# Done\n\n## Feed\n\n- [x] Old @done(2026-09-01)\n");
        ensure_section(&mut doc, "Work").unwrap();
        let out = render(&doc);
        assert!(
            out.find("## Work").unwrap() < out.find("# Done").unwrap(),
            "got:\n{out}"
        );
        // Neither — end of file:
        let mut doc = parse("# Feed\n\n- [ ] A\n");
        let at = ensure_section(&mut doc, "Work").unwrap();
        assert_eq!(at, doc.nodes.len());
        assert!(
            render(&doc).ends_with("## Work\n\n"),
            "got:\n{}",
            render(&doc)
        );
    }

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

    #[test]
    fn ensure_section_rejects_reserved_and_ignores_archive_sections() {
        let mut doc = parse("# Feed\n\n# Done\n\n## Chores\n\n- [x] C @done(2026-09-01)\n");
        assert!(matches!(
            ensure_section(&mut doc, "Agent"),
            Err(OpError::Reserved(_))
        ));
        // "Chores" exists only in the archive → a NEW active section is created:
        ensure_section(&mut doc, "Chores").unwrap();
        let out = render(&doc);
        assert!(
            out.find("## Chores").unwrap() < out.find("# Done").unwrap(),
            "got:\n{out}"
        );
    }

    #[test]
    fn ensure_section_rejects_empty_name() {
        let mut doc = parse("# Feed\n\n- [ ] A\n");
        let before = render(&doc);
        assert!(matches!(
            ensure_section(&mut doc, "  "),
            Err(OpError::EmptyName)
        ));
        assert_eq!(render(&doc), before);
    }

    #[test]
    fn move_to_section_preserves_state_body_and_tokens() {
        let mut doc =
            parse("# Feed\n\n- [~] A @agent(claude:abc)\n  ctx line\n\n## Work\n\n- [ ] W\n");
        let i = find(&doc, "A").unwrap();
        move_to_section(&mut doc, i, Some("Work")).unwrap();
        let out = render(&doc);
        assert!(
            out.contains("- [ ] W\n- [~] A @agent(claude:abc)\n  ctx line\n"),
            "got:\n{out}"
        );
    }

    #[test]
    fn move_to_section_creates_target_and_keeps_emptied_heading() {
        let mut doc = parse("# Feed\n\n## Work\n\n- [ ] Only\n\n## Agent\n");
        let i = find(&doc, "Only").unwrap();
        move_to_section(&mut doc, i, Some("Chores")).unwrap();
        let out = render(&doc);
        assert!(
            out.contains("## Work"),
            "emptied heading must survive:\n{out}"
        );
        assert!(out.contains("## Chores\n\n- [ ] Only\n"), "got:\n{out}");
        assert!(
            out.find("## Chores").unwrap() < out.find("## Agent").unwrap(),
            "got:\n{out}"
        );
    }

    #[test]
    fn move_to_none_lands_in_first_human_section_and_reserved_errors() {
        let mut doc = parse("# Feed\n\n- [ ] A\n\n## Work\n\n- [ ] W\n");
        let w = find(&doc, "W").unwrap();
        move_to_section(&mut doc, w, None).unwrap();
        assert!(
            render(&doc).contains("- [ ] A\n- [ ] W\n"),
            "got:\n{}",
            render(&doc)
        );
        // Reserved target: error, document untouched.
        let before = render(&doc);
        let a = find(&doc, "A").unwrap();
        assert!(matches!(
            move_to_section(&mut doc, a, Some("Done")),
            Err(OpError::Reserved(_))
        ));
        assert_eq!(render(&doc), before);
    }

    #[test]
    fn move_to_section_rejects_empty_name_without_dropping_item() {
        let mut doc = parse("# Feed\n\n- [ ] A\n\n## Work\n\n- [ ] W\n");
        let before = render(&doc);
        let a = find(&doc, "A").unwrap();
        assert!(matches!(
            move_to_section(&mut doc, a, Some("  ")),
            Err(OpError::EmptyName)
        ));
        assert_eq!(render(&doc), before);
    }

    #[test]
    fn move_to_none_reaches_empty_uncategorized_region() {
        // Single categorized item; clearing its category must lift it out.
        let mut doc = parse("# Feed\n\n## Work\n\n- [ ] W\n");
        let w = find(&doc, "W").unwrap();
        move_to_section(&mut doc, w, None).unwrap();
        let out = render(&doc);
        assert!(
            out.find("- [ ] W").unwrap() < out.find("## Work").unwrap(),
            "item must land before the first section, got:\n{out}"
        );
    }

    #[test]
    fn move_to_none_never_lands_in_another_category() {
        let mut doc = parse("# Feed\n\n## Work\n\n- [ ] W\n\n## Chores\n\n- [ ] C\n");
        let c = find(&doc, "C").unwrap();
        move_to_section(&mut doc, c, None).unwrap();
        let out = render(&doc);
        assert!(
            out.find("- [ ] C").unwrap() < out.find("## Work").unwrap(),
            "cleared item must precede all sections, got:\n{out}"
        );
    }

    #[test]
    fn move_to_section_reserved_with_padding_errors_and_leaves_doc_untouched() {
        let mut doc = parse("# Feed\n\n- [ ] A\n\n## Work\n\n- [ ] W\n");
        let before = render(&doc);
        let a = find(&doc, "A").unwrap();
        assert!(matches!(
            move_to_section(&mut doc, a, Some(" done ")),
            Err(OpError::Reserved(_))
        ));
        assert_eq!(render(&doc), before);
    }
}
