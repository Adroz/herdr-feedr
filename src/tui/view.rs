use crate::feed::State;
use crate::tui::app::{App, Row};
use crate::tui::modal::{EditFocus, Modal};
use crate::tui::socket::AgentStatus;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

/// Toolbar text; the click spans in input.rs must match these columns:
/// « at 0, "sweep" at 2..=6, "file" at 8..=11.
pub const TOOLBAR: &str = "« sweep file";

pub fn state_glyph(s: State) -> char {
    match s {
        State::Open => '·',
        State::InProgress => '~',
        State::Review => '?',
        State::Done => 'x',
    }
}

/// Display-only live status (spec §3: never written to the file).
/// Unknown renders nothing — it must not look like done.
pub fn status_glyph(s: AgentStatus) -> Option<char> {
    match s {
        AgentStatus::Working => Some('>'),
        AgentStatus::Blocked => Some('!'),
        AgentStatus::Done => Some('✓'),
        AgentStatus::Idle => Some('-'),
        AgentStatus::Unknown => None,
    }
}

pub fn ellipsize(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else if width == 0 {
        String::new()
    } else {
        let mut t: String = s.chars().take(width - 1).collect();
        t.push('…');
        t
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if app.collapsed {
        // ~3-col rail: click anywhere restores (input.rs).
        f.render_widget(Paragraph::new("»"), area);
        return;
    }
    let [toolbar, list, done, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(TOOLBAR), toolbar);

    let w = area.width as usize;
    let lines: Vec<Line> = app
        .rows
        .iter()
        .skip(app.scroll)
        .take(list.height as usize)
        .map(|r| row_line(app, r, w))
        .collect();
    f.render_widget(Paragraph::new(lines), list);

    f.render_widget(
        Paragraph::new(format!("Done ({})", app.done_count))
            .style(Style::default().add_modifier(Modifier::BOLD)),
        done,
    );
    f.render_widget(
        Paragraph::new(app.status_msg.clone().unwrap_or_default()),
        status,
    );

    draw_modal(f, app);
}

fn draw_modal(f: &mut Frame, app: &App) {
    match &app.modal {
        Modal::None => {}
        Modal::Edit(m) => {
            let area = centered(f.area(), 90, 80);
            f.render_widget(Clear, area);
            let outer = Block::bordered().title(if m.original.is_some() {
                "Edit item"
            } else {
                "New item"
            });
            let inner = outer.inner(area);
            f.render_widget(outer, area);
            let [section, title, body, hints] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(1),
            ])
            .areas(inner);
            if m.original.is_none() {
                let marker = if m.focus == EditFocus::Section {
                    "*"
                } else {
                    ""
                };
                f.render_widget(
                    Paragraph::new(format!("Section{marker}: ‹ {} ›", m.choice_label())),
                    section,
                );
            }
            f.render_widget(&m.title, title);
            f.render_widget(&m.body, body);
            let hint = if m.original.is_some() {
                "Tab field · ^S save · ^D delete · Esc cancel"
            } else {
                "Tab field · ^S save · Esc cancel"
            };
            f.render_widget(Paragraph::new(hint), hints);
        }
        Modal::ConfirmDelete(m) => {
            let area = centered(f.area(), 80, 20);
            f.render_widget(Clear, area);
            let outer = Block::bordered().title("Confirm delete");
            let inner = outer.inner(area);
            f.render_widget(outer, area);
            let title = m
                .original
                .as_ref()
                .map(|k| k.title.clone())
                .unwrap_or_default();
            f.render_widget(
                Paragraph::new(format!("Delete \"{title}\"? [y]es / [n]o")),
                inner,
            );
        }
    }
}

fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let [_, mid_v, _] = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .areas(area);
    let [_, mid, _] = Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .areas(mid_v);
    mid
}

fn row_line(app: &App, row: &Row, w: usize) -> Line<'static> {
    match row {
        Row::Section(t) => Line::styled(
            ellipsize(t, w),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Row::Item { key, .. } => Line::raw(format!(
            "{} {}",
            state_glyph(key.state),
            ellipsize(&key.title, w.saturating_sub(2)),
        )),
        Row::AgentLine { agent, .. } => {
            let glyph = app
                .statuses
                .get(&agent.id)
                .and_then(|i| status_glyph(i.status))
                .map(|g| format!(" {g}"))
                .unwrap_or_default();
            Line::raw(format!(
                "  @{}{}",
                ellipsize(&agent.kind, w.saturating_sub(4 + glyph.chars().count())),
                glyph,
            ))
        }
        Row::Add => Line::raw("+ add"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Side, SidebarConfig};
    use crate::feed::parse::parse;
    use crate::tui::app::App;
    use crate::tui::socket::{AgentInfo, AgentStatus, FakeHerdr};
    use crate::tui::test_util::render_to_strings;

    fn test_cfg() -> SidebarConfig {
        SidebarConfig {
            side: Side::Left,
            width: 0.18,
            auto_dock: false,
        }
    }

    fn app_with(text: &str) -> App {
        let mut app = App::new(
            "/nonexistent/feed.md".into(),
            test_cfg(),
            Box::new(FakeHerdr::default()),
        );
        app.doc = parse(text);
        app.rebuild();
        app
    }

    const SAMPLE: &str = "\
# Feed

- [ ] Fix auth redirect loop
- [~] Migrate CI @agent(claude:0198f3ab)

## Agent

- [?] Add retry to deploy test @agent(claude:0198f3ab)

# Done

## Feed

- [x] Old thing @done(2026-09-01)
";

    #[test]
    fn renders_toolbar_list_done_and_status_rows() {
        let mut app = app_with(SAMPLE);
        app.statuses.insert(
            "0198f3ab".into(),
            AgentInfo {
                pane_id: "w1:p7".into(),
                kind: "claude".into(),
                session_id: "0198f3ab".into(),
                status: AgentStatus::Working,
            },
        );
        app.status_msg = Some("hello".into());
        let rows = render_to_strings(&app, 20, 12);
        assert_eq!(rows[0], "« sweep file");
        assert_eq!(rows[1], "· Fix auth redirect…"); // ellipsized at 20 cols
        assert_eq!(rows[2], "~ Migrate CI");
        assert_eq!(rows[3], "  @claude >"); // live working glyph
        assert_eq!(rows[4], "Agent");
        assert_eq!(rows[5], "? Add retry to depl…");
        assert_eq!(rows[6], "  @claude >");
        assert_eq!(rows[7], "+ add");
        assert_eq!(rows[10], "Done (1)"); // bottom-pinned, h-2
        assert_eq!(rows[11], "hello"); // status line, h-1
    }

    #[test]
    fn agent_glyph_hidden_when_socket_absent() {
        let app = app_with(SAMPLE); // statuses empty
        let rows = render_to_strings(&app, 20, 12);
        assert_eq!(rows[3], "  @claude"); // sub-line still shown, no glyph
    }

    #[test]
    fn collapsed_renders_rail() {
        let mut app = app_with(SAMPLE);
        app.collapsed = true;
        let rows = render_to_strings(&app, 3, 6);
        assert_eq!(rows[0], "»");
        assert!(rows[1..].iter().all(|r| r.is_empty()));
    }

    #[test]
    fn list_scrolls() {
        let mut app = app_with(SAMPLE);
        app.scroll = 2;
        let rows = render_to_strings(&app, 20, 12);
        assert_eq!(rows[1], "  @claude"); // row index 2 of the row model
    }

    #[test]
    fn ellipsize_is_char_safe() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("exactly-ten", 11), "exactly-ten");
        assert_eq!(ellipsize("exactly-eleven!", 11), "exactly-el…");
        assert_eq!(ellipsize("x", 0), "");
    }
}
