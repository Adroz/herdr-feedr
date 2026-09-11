use super::*;

pub fn parse(text: &str) -> Document {
    let mut nodes: Vec<Node> = Vec::new();
    for line in text.lines() {
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

fn split_trailing_token(s: &str) -> Option<(&str, Token)> {
    let open = s.rfind("@agent(").or(s.rfind("@done("))?;
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
}
