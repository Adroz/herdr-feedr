use super::*;
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
                if item.title.is_empty() && item.agent.is_none() && item.done_date.is_none() {
                    // "- [ ]" alone re-parses as Raw, losing item-ness and
                    // detaching the body. Keep the trailing space so it
                    // still parses as an item.
                    out.push(' ');
                }
                out.push('\n');
                for b in &item.body {
                    if !b.is_empty() {
                        out.push_str("  ");
                        out.push_str(b);
                    }
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
    use std::io::Write as _;
    tmp.write_all(render(doc).as_bytes())?;
    if let Ok(meta) = std::fs::metadata(path) {
        std::fs::set_permissions(tmp.path(), meta.permissions())?;
    }
    tmp.persist(path)?;
    Ok(())
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

    #[test]
    fn empty_title_tokenless_item_round_trips() {
        assert_eq!(render(&parse("- [ ] \n")), "- [ ] \n");
    }

    #[test]
    fn body_with_blank_line_round_trips() {
        let text = "- [ ] Task\n  ```sh\n  echo one\n\n  echo two\n  ```\n";
        assert_eq!(render(&parse(text)), text);
    }

    #[cfg(unix)]
    #[test]
    fn save_atomic_overwrites_and_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.md");
        std::fs::write(&path, "- [ ] Old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        save_atomic(&parse("- [ ] New\n"), &path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] New\n");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
    }
}
