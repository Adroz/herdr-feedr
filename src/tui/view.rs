use crate::feed::State;
use crate::tui::app::{App, Row};
use crate::tui::modal::{self, EditFocus, Modal};
use crate::tui::socket::AgentStatus;
use crate::tui::theme;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
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

/// Truncate `s` to fit within `width` terminal cells, appending `…` when it
/// doesn't fit. Budgets by display width (via `unicode-width`), not char
/// count — a char-count budget lets wide CJK/emoji glyphs overflow their
/// cell budget and clips the `…` marker itself (Task 5 review carry-forward).
pub fn ellipsize(s: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let total_width: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total_width <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    // Reserve 1 cell for the ellipsis marker itself (width 1).
    let budget = width - 1;
    let mut acc = 0usize;
    let mut t = String::new();
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if acc + cw > budget {
            break;
        }
        acc += cw;
        t.push(c);
    }
    t.push('…');
    t
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if app.collapsed {
        // ~3-col rail: click anywhere restores (input.rs).
        f.render_widget(Paragraph::new("»").style(theme::muted_row()), area);
        return;
    }
    let [toolbar, list, done, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(TOOLBAR).style(theme::muted_row()), toolbar);

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
            .style(theme::muted_row().add_modifier(Modifier::BOLD)),
        done,
    );
    f.render_widget(
        Paragraph::new(app.status_msg.clone().unwrap_or_default()).style(theme::muted_row()),
        status,
    );

    draw_modal(f, app);
}

fn draw_modal(f: &mut Frame, app: &App) {
    match &app.modal {
        Modal::None => {}
        Modal::Edit(m) => draw_edit_modal(f, m),
        Modal::ConfirmDelete(m) => {
            let area = modal::centered(f.area(), 80, 20);
            f.render_widget(Clear, area);
            let outer = Block::bordered()
                .border_style(theme::modal_border())
                .title("Confirm delete");
            let inner = outer.inner(area);
            f.render_widget(outer, area);
            let title = m
                .original
                .as_ref()
                .map(|k| k.title.clone())
                .unwrap_or_default();
            f.render_widget(
                Paragraph::new(format!("Delete \"{title}\"? [y]es / [n]o"))
                    .style(theme::normal_text()),
                inner,
            );
        }
        Modal::DoneView { scroll } => {
            draw_viewer(f, "Done archive — Esc closes", &app.archive_text(), *scroll)
        }
        Modal::FileView { scroll } => draw_viewer(
            f,
            "feed file — e edits in $EDITOR, Esc closes",
            &app.file_text(),
            *scroll,
        ),
    }
}

/// Draw the edit/create modal from its shared pure geometry (`modal::edit_layout`
/// — the same function `modal::step`'s mouse handling hit-tests against, so
/// the drawn boxes and the clickable ones can never drift apart), render the
/// Save/Cancel/Delete buttons, and — when a textarea is focused — place the
/// real terminal cursor there so the terminal's own blink applies (spec:
/// replace the static styled cursor cell).
fn draw_edit_modal(f: &mut Frame, m: &modal::EditModal) {
    let is_edit = m.original.is_some();
    let layout = modal::edit_layout(f.area(), is_edit);

    f.render_widget(Clear, layout.outer);
    let outer = Block::bordered()
        .border_style(theme::modal_border())
        .title(if is_edit { "Edit item" } else { "New item" });
    f.render_widget(outer, layout.outer);

    if !is_edit {
        let marker = if m.focus == EditFocus::Section {
            "*"
        } else {
            ""
        };
        f.render_widget(
            Paragraph::new(format!("Add to{marker}: ‹ {} ›", m.choice_label()))
                .style(theme::normal_text()),
            layout.section,
        );
    }
    f.render_widget(&m.title, layout.title);
    f.render_widget(&m.body, layout.body);

    render_button(
        f,
        layout.save,
        modal::SAVE_LABEL,
        m.focus == EditFocus::Save,
    );
    render_button(
        f,
        layout.cancel,
        modal::CANCEL_LABEL,
        m.focus == EditFocus::Cancel,
    );
    if let Some(delete) = layout.delete {
        render_button(f, delete, modal::DELETE_LABEL, m.focus == EditFocus::Delete);
    }

    let hint = if is_edit {
        "Tab field · ^S save · ^D delete · Esc cancel"
    } else {
        "Tab field · ^S save · Esc cancel"
    };
    f.render_widget(Paragraph::new(hint).style(theme::muted_row()), layout.hints);

    match m.focus {
        EditFocus::Title => {
            f.set_cursor_position(cursor_screen_pos(layout.title, m.title.cursor()))
        }
        EditFocus::Body => f.set_cursor_position(cursor_screen_pos(layout.body, m.body.cursor())),
        _ => {}
    }
}

fn render_button(f: &mut Frame, area: Rect, label: &str, focused: bool) {
    f.render_widget(Paragraph::new(label).style(theme::button(focused)), area);
}

/// Screen position of a textarea's cursor, given the bordered `Rect` it was
/// rendered into. tui-textarea 0.7 doesn't expose its internal scroll
/// viewport publicly, so this clamps into the visible inner rect rather than
/// tracking exact horizontal/vertical scroll offsets — acceptable for these
/// modal fields (title is a single line; body is short).
fn cursor_screen_pos(area: Rect, cursor: (usize, usize)) -> Position {
    let inner = Block::bordered().inner(area);
    let (row, col) = cursor;
    Position {
        x: inner.x + (col as u16).min(inner.width.saturating_sub(1)),
        y: inner.y + (row as u16).min(inner.height.saturating_sub(1)),
    }
}

fn draw_viewer(f: &mut Frame, title: &str, text: &str, scroll: u16) {
    let area = modal::centered(f.area(), 90, 80);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(text.to_string())
            .style(theme::normal_text())
            .block(
                Block::bordered()
                    .border_style(theme::modal_border())
                    .title(title.to_string()),
            )
            .scroll((scroll, 0)),
        area,
    );
}

fn row_line(app: &App, row: &Row, w: usize) -> Line<'static> {
    match row {
        Row::Section(t) => Line::styled(ellipsize(t, w), theme::section_header()),
        Row::Item { key, .. } => {
            let title = ellipsize(&key.title, w.saturating_sub(2));
            Line::from(vec![
                Span::styled(
                    format!("{} ", state_glyph(key.state)),
                    theme::item_glyph(key.state),
                ),
                Span::styled(title, theme::normal_text()),
            ])
        }
        Row::AgentLine { agent, .. } => {
            let status = app.statuses.get(&agent.id).map(|i| i.status);
            let glyph_char = status.and_then(status_glyph);
            let glyph = glyph_char.map(|g| format!(" {g}")).unwrap_or_default();
            let kind = ellipsize(&agent.kind, w.saturating_sub(4 + glyph.chars().count()));
            let mut spans = vec![Span::styled(format!("  @{kind}"), theme::agent_subline())];
            if let (Some(g), Some(s)) = (glyph_char, status) {
                spans.push(Span::styled(format!(" {g}"), theme::agent_status(s)));
            }
            Line::from(spans)
        }
        Row::Add => Line::styled("+ add", theme::normal_text()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Side, SidebarConfig};
    use crate::feed::parse::parse;
    use crate::tui::app::{Action, App, ItemKey};
    use crate::tui::socket::{AgentInfo, AgentStatus, FakeHerdr};
    use crate::tui::test_util::{render_cursor, render_to_strings};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

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

    #[test]
    fn ellipsize_budgets_by_display_width_not_char_count() {
        use unicode_width::UnicodeWidthStr;
        // CJK chars are 2 cells wide each; a char-count budget of 10 lets 9
        // of them through (18 cells) plus the ellipsis — badly overflowing a
        // 10-cell pane. The display-width budget must keep the whole result
        // within `width` cells.
        let cjk = "測試標題看看看看看看"; // 10 chars, 20 cells if unclipped
        let out = ellipsize(cjk, 10);
        assert!(
            out.ends_with('…'),
            "result should end with ellipsis: {out:?}"
        );
        assert!(
            out.width() <= 10,
            "result {out:?} has display width {} > budget 10",
            out.width()
        );
    }

    #[test]
    fn cursor_screen_pos_clamps_within_inner_rect() {
        let area = Rect::new(5, 5, 10, 3); // bordered rect; inner is (6,6,8,1)
        assert_eq!(cursor_screen_pos(area, (0, 0)), Position { x: 6, y: 6 });
        // Cursor past the visible inner rect clamps to its last cell rather
        // than escaping the modal or panicking.
        assert_eq!(cursor_screen_pos(area, (50, 50)), Position { x: 13, y: 6 });
    }

    #[test]
    fn cursor_hidden_when_no_modal_active() {
        let app = app_with(SAMPLE);
        assert_eq!(render_cursor(&app, 40, 12), None);
    }

    #[test]
    fn cursor_visible_at_title_when_create_modal_focuses_title() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenCreate);
        // Section is focused first on create; Tab moves to Title.
        app.handle_modal_event(
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            (40, 12),
        );
        let pos =
            render_cursor(&app, 40, 12).expect("cursor must be visible when Title is focused");
        let layout = modal::edit_layout(Rect::new(0, 0, 40, 12), false);
        let inner = Block::bordered().inner(layout.title);
        // Untouched textarea: cursor still at its origin (row 0, col 0).
        assert_eq!(
            pos,
            Position {
                x: inner.x,
                y: inner.y
            }
        );
    }

    #[test]
    fn create_modal_shows_add_to_label_and_save_cancel_but_no_delete() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenCreate);
        let rows = render_to_strings(&app, 60, 20);
        let joined = rows.join("\n");
        assert!(joined.contains("Add to"), "got:\n{joined}");
        assert!(joined.contains(modal::SAVE_LABEL), "got:\n{joined}");
        assert!(joined.contains(modal::CANCEL_LABEL), "got:\n{joined}");
        assert!(!joined.contains(modal::DELETE_LABEL), "got:\n{joined}");
    }

    #[test]
    fn edit_modal_shows_delete_button() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenEdit(ItemKey {
            title: "Fix auth redirect loop".into(),
            state: State::Open,
        }));
        let rows = render_to_strings(&app, 60, 20);
        let joined = rows.join("\n");
        assert!(joined.contains(modal::DELETE_LABEL), "got:\n{joined}");
    }
}
