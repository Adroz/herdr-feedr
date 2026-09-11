use super::*;

/// Lossless for lines the parser does not own (they become [`Node::Raw`] and
/// re-emit verbatim). Recognized lines (headings, items, bodies) are
/// normalized: trailing whitespace is trimmed and token spacing and order are
/// canonicalized (agent before done), so render∘parse is byte-exact only for
/// canonical input.
pub fn parse(text: &str) -> Document {
    let lines: Vec<&str> = text.lines().collect();
    let mut nodes: Vec<Node> = Vec::new();
    let mut idx = 0usize;
    while idx < lines.len() {
        let line = lines[idx];
        if let Some(rest) = line.strip_prefix("## ") {
            nodes.push(Node::Heading {
                level: 2,
                text: rest.trim().to_string(),
            });
        } else if let Some(rest) = line.strip_prefix("# ") {
            nodes.push(Node::Heading {
                level: 1,
                text: rest.trim().to_string(),
            });
        } else if let Some(item) = parse_item_line(line) {
            nodes.push(Node::Item(item));
        } else if line.starts_with("  ") && !line.trim().is_empty() {
            // Body line: attach to the most recent item if one directly precedes
            // (an item's body is contiguous).
            if let Some(Node::Item(item)) = nodes.last_mut() {
                item.body.push(line[2..].to_string());
            } else {
                nodes.push(Node::Raw(line.to_string()));
            }
        } else if line.trim().is_empty() {
            // A blank line is a BODY line (stored as "") only when it sits
            // between an item's body content and a following indented body
            // line — e.g. a blank line inside a fenced code block or between
            // paragraphs. Otherwise it's a Raw line (blank lines that end a
            // body, or that sit between unrelated nodes, stay Raw).
            let preceding_is_body_context =
                matches!(nodes.last(), Some(Node::Item(it)) if !it.body.is_empty());
            let next_is_body_line = lines[idx + 1..]
                .iter()
                .find(|l| !l.trim().is_empty())
                .is_some_and(|l| l.starts_with("  ") && !l.trim().is_empty());
            if preceding_is_body_context && next_is_body_line {
                if let Some(Node::Item(item)) = nodes.last_mut() {
                    item.body.push(String::new());
                }
            } else {
                nodes.push(Node::Raw(line.to_string()));
            }
        } else {
            nodes.push(Node::Raw(line.to_string()));
        }
        idx += 1;
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
                Token::Agent(a) if agent.is_none() => agent = Some(a),
                Token::Done(d) if done_date.is_none() => done_date = Some(d),
                _ => {
                    title = t.to_string();
                    break;
                }
            }
            title = head.trim_end().to_string();
        } else {
            title = t.to_string();
            break;
        }
    }
    Some(Item {
        state,
        title,
        agent,
        done_date,
        body: Vec::new(),
    })
}

enum Token {
    Agent(AgentRef),
    Done(String),
}

/// A malformed rightmost token (e.g. `@agent(nocolon)`) returns None, which
/// deliberately shields any earlier valid tokens: the whole tail stays in the
/// title verbatim rather than being partially consumed. Lossless over clever.
fn split_trailing_token(s: &str) -> Option<(&str, Token)> {
    let a = s.rfind("@agent(");
    let d = s.rfind("@done(");
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
        let items: Vec<&Item> = doc
            .nodes
            .iter()
            .filter_map(|n| match n {
                Node::Item(i) => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(items.len(), 6);
        assert_eq!(items[0].title, "Fix auth redirect loop");
        assert_eq!(items[0].state, State::Open);
        assert_eq!(items[0].body, vec!["Repro: bounces forever.", "See #142."]);
        assert_eq!(
            items[1].agent.as_ref().unwrap().to_string(),
            "claude:0198f3ab"
        );
        assert_eq!(items[1].title, "Migrate CI");
        assert_eq!(items[5].done_date.as_deref(), Some("2026-09-01"));
        let headings: Vec<(u8, &str)> = doc
            .nodes
            .iter()
            .filter_map(|n| match n {
                Node::Heading { level, text } => Some((*level, text.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            headings,
            vec![
                (1, "Feed"),
                (2, "Later"),
                (2, "Agent"),
                (1, "Done"),
                (2, "Feed")
            ]
        );
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

    #[test]
    fn strips_both_tokens_regardless_of_order() {
        let doc = parse("- [x] Foo @agent(claude:123) @done(2026-09-01)\n");
        match &doc.nodes[0] {
            Node::Item(it) => {
                assert_eq!(it.title, "Foo");
                assert_eq!(it.agent.as_ref().unwrap().to_string(), "claude:123");
                assert_eq!(it.done_date.as_deref(), Some("2026-09-01"));
            }
            n => panic!("expected item, got {n:?}"),
        }
    }

    #[test]
    fn blank_lines_inside_body_stay_in_body() {
        let text = "- [ ] Task\n  para one\n\n  para two\n";
        let doc = parse(text);
        match &doc.nodes[0] {
            Node::Item(it) => assert_eq!(it.body, vec!["para one", "", "para two"]),
            n => panic!("expected item, got {n:?}"),
        }
        assert_eq!(doc.nodes.len(), 1);
    }

    #[test]
    fn trailing_blank_after_body_is_raw() {
        let doc = parse("- [ ] Task\n  body\n\n- [ ] Next\n");
        assert_eq!(doc.nodes.len(), 3); // item, Raw(""), item
        assert!(matches!(&doc.nodes[1], Node::Raw(s) if s.is_empty()));
    }

    #[test]
    fn duplicate_tokens_stay_in_title() {
        let doc = parse("- [x] A @done(2026-01-01) @done(2026-02-02)\n");
        match &doc.nodes[0] {
            Node::Item(it) => {
                assert_eq!(it.title, "A @done(2026-01-01)");
                assert_eq!(it.done_date.as_deref(), Some("2026-02-02"));
            }
            n => panic!("expected item, got {n:?}"),
        }
    }

    #[test]
    fn blank_before_any_body_line_is_raw() {
        let doc = parse("- [ ] A\n\n  late body\n");
        match &doc.nodes[0] {
            Node::Item(it) => assert!(it.body.is_empty(), "body must be empty, got {:?}", it.body),
            n => panic!("expected item, got {n:?}"),
        }
        assert!(matches!(&doc.nodes[1], Node::Raw(s) if s.is_empty()));
        assert!(matches!(&doc.nodes[2], Node::Raw(_))); // "  late body" detached
    }
}
