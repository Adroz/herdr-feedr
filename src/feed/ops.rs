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
        body: body.iter().filter(|l| !l.is_empty()).cloned().collect(),
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

fn agent_section_end(doc: &Document) -> Option<usize> {
    let start = doc.nodes.iter().position(
        |n| matches!(n, Node::Heading { level: 2, text } if text.eq_ignore_ascii_case("Agent")),
    )?;
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
    fn add_filters_empty_body_lines() {
        let mut doc = parse("- [ ] A\n");
        add(
            &mut doc,
            "B",
            &["one".into(), "".into(), "two".into()],
            Zone::Human,
        );
        let i = find(&doc, "B").unwrap();
        match &doc.nodes[i] {
            Node::Item(it) => assert_eq!(it.body, vec!["one", "two"]),
            n => panic!("expected item, got {n:?}"),
        }
    }
}
