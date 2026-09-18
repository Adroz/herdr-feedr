use crate::tui::app::{App, Row};
use crate::tui::modal::{self, EditFocus, Modal};
use crate::tui::socket::AgentStatus;
use crate::tui::theme;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph};
use ratatui::Frame;

/// Toolbar text; the click span in input.rs must match these columns:
/// "clear completed" at 0..=14. Round-3 item 2 moved the collapse chevron
/// off the toolbar onto the bottom-right of the status row
/// (`draw_status_row`) — herdr's own native position for it. Round-4 item 1
/// dropped "file" from the toolbar entirely; the internal file viewer stays
/// reachable via the `f` key (input.rs `translate`).
pub const TOOLBAR: &str = "clear completed";

/// Collapse-chevron glyph, pinned bottom-right of the status row (round-3
/// item 2). Same glyph the toolbar used to show at column 0.
const COLLAPSE_CHEVRON: &str = "«";

/// Width, in cells, of the chevron's click zone at the right edge of the
/// status row (input.rs hit-tests the same width) — one cell of breathing
/// room plus the glyph itself, mirroring the toolbar's own multi-cell click
/// zones rather than a single hard-to-hit column.
const CHEVRON_ZONE_WIDTH: u16 = 2;

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

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if app.collapsed {
        // ~3-col rail: click anywhere restores (input.rs).
        render_collapsed_rail(f, area, app);
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
    draw_status_row(f, status, app);

    draw_modal(f, app);
}

/// Round-5 (Plan 3c): the collapsed rail is a vertical status strip, not
/// blank space. Top to bottom in the ~1-cell inner column: the open-task
/// count as stacked digits (normal text — this is the headline number, not
/// chrome), a blank, a mauve `?` + stacked count when any `[?]` item exists,
/// a blank, a red `!` when any linked agent is Blocked, then the `»` restore
/// chevron pinned to the bottom row (unchanged from round-4 — click anywhere
/// restores, input.rs). Indicators that don't apply are omitted outright
/// (no reserved blank slot for them) — an all-clear feed shows just the open
/// count and the chevron, with real blank space between.
fn render_collapsed_rail(f: &mut Frame, area: Rect, app: &App) {
    let mut lines: Vec<Line> = Vec::new();
    for d in stacked_digits(app.open_task_count()) {
        lines.push(Line::styled(d, theme::normal_text()));
    }
    let review = app.review_task_count();
    if review > 0 {
        let review_style = theme::item_glyph(crate::feed::State::Review); // mauve
        lines.push(Line::from(""));
        lines.push(Line::styled("?", review_style));
        for d in stacked_digits(review) {
            lines.push(Line::styled(d, review_style));
        }
    }
    if app.any_linked_agent_blocked() {
        lines.push(Line::from(""));
        lines.push(Line::styled("!", theme::agent_status(AgentStatus::Blocked)));
        // red
    }
    // Pad/truncate so the chevron always lands on the bottom row, same as
    // the plain-blank rail before it.
    let body_rows = area.height.saturating_sub(1) as usize;
    while lines.len() < body_rows {
        lines.push(Line::from(""));
    }
    lines.truncate(body_rows);
    lines.push(Line::styled("»", theme::muted_row()));
    f.render_widget(Paragraph::new(lines).style(theme::muted_row()), area);
}

/// A count's decimal digits, each as its own single-character string — one
/// digit per rendered row (e.g. 12 -> ["1", "2"]).
fn stacked_digits(n: usize) -> Vec<String> {
    n.to_string().chars().map(|c| c.to_string()).collect()
}

/// Status row: the status message on the left, the collapse chevron pinned
/// to the bottom-right corner (round-3 item 2 — herdr's own native position
/// for its collapse control). The chevron always owns the rightmost
/// `CHEVRON_ZONE_WIDTH` cells; a status message that would run into that
/// zone is truncated first so the chevron never gets clipped or overwritten.
fn draw_status_row(f: &mut Frame, area: Rect, app: &App) {
    let chevron_w = CHEVRON_ZONE_WIDTH.min(area.width);
    let text_w = area.width - chevron_w;
    let msg = app.status_msg.as_deref().unwrap_or("");
    f.render_widget(
        Paragraph::new(ellipsize(msg, text_w as usize)).style(theme::muted_row()),
        Rect {
            width: text_w,
            ..area
        },
    );
    if chevron_w > 0 {
        let chevron_area = Rect {
            x: area.x + text_w,
            width: chevron_w,
            ..area
        };
        f.render_widget(
            Paragraph::new(COLLAPSE_CHEVRON)
                .style(theme::muted_row())
                .alignment(Alignment::Right),
            chevron_area,
        );
    }
}

/// Round-3 item 1: every modal reads as a true catppuccin-style overlay —
/// before the panel itself is drawn, the whole frame is repainted as a
/// dimmed backdrop (`theme::backdrop`: `Modifier::DIM` + `theme::BASE` bg).
/// Each panel below then draws `Clear` over its own rect first (resetting
/// style/modifiers there) followed by a `Block` styled with
/// `theme::modal_panel_style` (solid `theme::MANTLE` bg), so the panel
/// itself — and everything drawn inside it — never carries the dim.
fn draw_modal(f: &mut Frame, app: &mut App) {
    if matches!(app.modal, Modal::None) {
        return;
    }
    dim_backdrop(f);
    // The edit modal needs `&mut` (per-frame viewport reset inside
    // `draw_edit_modal`); the read-only arms below borrow `app` immutably
    // (`archive_text`/`file_text`), so split the Edit arm out first.
    if let Modal::Edit(m) = &mut app.modal {
        draw_edit_modal(f, m);
        return;
    }
    match &app.modal {
        Modal::None | Modal::Edit(_) => {}
        Modal::ConfirmDelete(m) => {
            let area = modal::centered(f.area(), 80, 20);
            f.render_widget(Clear, area);
            let outer = Block::bordered()
                .style(theme::modal_panel_style())
                .border_style(theme::modal_border())
                .title_style(theme::modal_title())
                .title("Confirm delete")
                .padding(Padding::uniform(1));
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

/// Paint the entire frame with `theme::backdrop` (round-3 item 1). Every
/// modal branch below draws `Clear` over its own panel rect before drawing
/// the panel, which resets that rect's modifiers/colors — so only the area
/// outside the panel ends up dimmed.
fn dim_backdrop(f: &mut Frame) {
    let area = f.area();
    f.buffer_mut().set_style(area, theme::backdrop());
}

/// Draw the edit/create modal from its shared pure geometry (`modal::edit_layout`
/// — the same function `modal::step`'s mouse handling hit-tests against, so
/// the drawn boxes and the clickable ones can never drift apart) and render
/// the Save/Cancel/Delete buttons. The focused textarea's own cursor cell
/// (styled reversed by `modal::EditModal::sync_blocks`) is the only cursor
/// shown — the real terminal cursor is never placed, because it would be
/// positioned from the textarea's logical cursor column, which drifts from
/// the drawn text whenever tui-textarea has scrolled its viewport
/// horizontally (see `sync_blocks` for the full rationale).
fn draw_edit_modal(f: &mut Frame, m: &mut modal::EditModal) {
    let is_edit = m.original.is_some();
    let layout = modal::edit_layout(f.area(), is_edit, m.claimed, m.dropdown_rows());

    // Re-derive each field's viewport from origin every frame: tui-textarea
    // only ever adjusts its viewport minimally to keep the cursor visible,
    // and never scrolls back when space frees up — so a scroll acquired at
    // the modal's first (narrow, pre-zoom) frame would otherwise stick for
    // the modal's whole life. Resetting before render makes the render's own
    // cursor-visibility pass compute the scroll fresh for the CURRENT size:
    // start-parked (unfocused) fields render unscrolled, and the focused
    // field's end-parked cursor is safe at any first-frame width.
    // (`i16::MIN + 1`, not `i16::MIN`: tui-textarea negates the delta
    // internally and `-i16::MIN` overflows. 32767 rows/cols is still far
    // beyond any feed item. And `scroll()` drags the CURSOR along to keep
    // it inside the scrolled viewport — verified live: without the
    // jump-back below, every frame teleported the cursor to the top-left
    // and typing landed at the start of the field — so the cursor is saved
    // and restored around the reset. `Jump` back to the same spot is a
    // no-op for an active selection: the anchor is kept and the cursor
    // ends where it already was.)
    for ta in [&mut m.category, &mut m.title, &mut m.body] {
        let (row, col) = ta.cursor();
        ta.scroll((i16::MIN + 1, i16::MIN + 1));
        ta.move_cursor(tui_textarea::CursorMove::Jump(row as u16, col as u16));
    }

    f.render_widget(Clear, layout.outer);
    let outer = Block::bordered()
        .style(theme::modal_panel_style())
        .border_style(theme::modal_border())
        .title_style(theme::modal_title())
        .title(if is_edit { "Edit item" } else { "New item" })
        .padding(Padding::uniform(1));
    f.render_widget(outer, layout.outer);

    f.render_widget(&m.category, layout.category);
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
    if let Some(release) = layout.release {
        render_button(
            f,
            release,
            modal::RELEASE_LABEL,
            m.focus == EditFocus::Release,
        );
    }
    if let Some(delete) = layout.delete {
        render_button(f, delete, modal::DELETE_LABEL, m.focus == EditFocus::Delete);
    }

    let hint = if is_edit {
        "Tab field · ^S save · ^D delete · Esc cancel"
    } else {
        "Tab field · ^S save · Esc cancel"
    };
    f.render_widget(Paragraph::new(hint).style(theme::muted_row()), layout.hints);

    // Dropdown last so it overlays the title/body area (mirrors
    // `edit_click`'s hit-test order in modal.rs). Bordered and Surface1-
    // tinted (distinct from the plain-text field it used to be) so it reads
    // as a floating popup rather than bare labels over the Title field.
    if layout.dropdown.height > 0 {
        f.render_widget(Clear, layout.dropdown);
        let dropdown_block = Block::bordered()
            .style(theme::dropdown_panel_style())
            .border_style(theme::dropdown_border());
        f.render_widget(dropdown_block, layout.dropdown);
        let rows: Vec<Line> = m
            .filtered()
            .iter()
            .take(layout.dropdown_inner.height as usize)
            .enumerate()
            .map(|(i, s)| Line::styled(s.clone(), theme::dropdown_row(m.dropdown == Some(i))))
            .collect();
        f.render_widget(
            Paragraph::new(rows).style(theme::dropdown_panel_style()),
            layout.dropdown_inner,
        );
    }
}

fn render_button(f: &mut Frame, area: Rect, label: &str, focused: bool) {
    f.render_widget(Paragraph::new(label).style(theme::button(focused)), area);
}

fn draw_viewer(f: &mut Frame, title: &str, text: &str, scroll: u16) {
    let area = modal::centered(f.area(), 90, 80);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(text.to_string())
            .style(theme::normal_text())
            .block(
                Block::bordered()
                    .style(theme::modal_panel_style())
                    .border_style(theme::modal_border())
                    .title_style(theme::modal_title())
                    .title(title.to_string())
                    .padding(Padding::uniform(1)),
            )
            .scroll((scroll, 0)),
        area,
    );
}

fn row_line(app: &App, row: &Row, w: usize) -> Line<'static> {
    match row {
        Row::Section(t) => Line::styled(ellipsize(t, w), theme::section_header()),
        Row::Item { key, .. } => {
            // "[<state-char>] " is a 4-cell prefix; only the char inside the
            // brackets carries state color, the brackets themselves are
            // normal text (round-2 item 2 — checkbox brackets, reusing
            // `State::to_char` rather than a separate glyph mapping).
            let title = ellipsize(&key.title, w.saturating_sub(4));
            Line::from(vec![
                Span::styled("[", theme::normal_text()),
                Span::styled(
                    key.state.to_char().to_string(),
                    theme::item_glyph(key.state),
                ),
                Span::styled("] ", theme::normal_text()),
                Span::styled(title, theme::normal_text()),
            ])
        }
        Row::AgentLine { agent, .. } => {
            let status = app.statuses.get(&agent.id).map(|i| i.status);
            // Presence in `statuses` IS liveness; the status only refines it.
            // An Unknown status must not fall back to "no marker", or a live
            // agent reads exactly like one that's gone (spec §3: absence of a
            // marker is neutral, never an assertion that the agent is dead).
            let glyph_char = status.and_then(|s| status_glyph(s).or(Some('·')));
            let glyph = glyph_char.map(|g| format!(" {g}")).unwrap_or_default();
            let kind = ellipsize(&agent.kind, w.saturating_sub(4 + glyph.chars().count()));
            let mut spans = vec![Span::styled(format!("  @{kind}"), theme::agent_subline())];
            if let (Some(g), Some(s)) = (glyph_char, status) {
                spans.push(Span::styled(format!(" {g}"), theme::agent_status(s)));
            }
            Line::from(spans)
        }
        // Round-2 item 3: de-emphasized like the toolbar/Done/status rows —
        // it's chrome, not a task.
        Row::Add => Line::styled("+ add", theme::muted_row()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Side, SidebarConfig};
    use crate::feed::parse::parse;
    use crate::feed::State;
    use crate::tui::app::{Action, App, ItemKey, Row};
    use crate::tui::socket::{AgentInfo, AgentStatus, FakeHerdr};
    use crate::tui::test_util::{render_buffer, render_cursor, render_to_strings};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    fn test_cfg() -> SidebarConfig {
        SidebarConfig {
            side: Side::Left,
            width: 0.18,
            max_width: 46,
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
        let rows = render_to_strings(&mut app, 20, 12);
        assert_eq!(rows[0], "clear completed"); // round-4: "file" removed from toolbar
        assert_eq!(rows[1], "[ ] Fix auth redire…"); // ellipsized at 20 cols
        assert_eq!(rows[2], "[~] Migrate CI");
        assert_eq!(rows[3], "  @claude >"); // live working glyph
        assert_eq!(rows[4], "Agent");
        assert_eq!(rows[5], "[?] Add retry to de…");
        assert_eq!(rows[6], "  @claude >");
        assert_eq!(rows[7], "+ add");
        assert_eq!(rows[10], "Done (1)"); // bottom-pinned, h-2
                                          // status line, h-1: "hello" on the left, « pinned bottom-right
                                          // (round-3 item 2 — native-herdr collapse position). 20-col row:
                                          // "hello" (5) + 14 blank cells + « (1) = 20.
        assert_eq!(rows[11], format!("hello{}«", " ".repeat(14)));
    }

    /// Round-3 item 2: the collapse chevron lives at the bottom-right of the
    /// status row now, not the toolbar. It occupies the rightmost 2 cells
    /// (a click zone, input.rs); the glyph itself renders in the very last
    /// cell.
    #[test]
    fn collapse_chevron_renders_bottom_right_of_status_row() {
        let mut app = app_with(SAMPLE);
        let rows = render_to_strings(&mut app, 20, 12);
        assert!(rows[11].ends_with('«'), "got: {:?}", rows[11]);
        assert_eq!(rows[11].chars().last(), Some('«'));
    }

    /// Round-3 item 2: a status message that would collide with the chevron
    /// zone is truncated so the chevron always wins the rightmost cell.
    #[test]
    fn long_status_message_truncates_before_chevron() {
        let mut app = app_with(SAMPLE);
        app.status_msg = Some("this is a very long status message that will not fit".into());
        let rows = render_to_strings(&mut app, 20, 12);
        assert_eq!(rows[11].chars().last(), Some('«'));
        assert_eq!(rows[11].chars().count(), 20);
    }

    /// Round-3 item 3: "sweep" is renamed "clear completed" with no leading
    /// chevron (that moved to the status row — item 2). Round-4 item 1:
    /// "file" is removed from the toolbar entirely — the file viewer is
    /// still reachable via the `f` key (input.rs).
    #[test]
    fn toolbar_text_matches_round4_spec() {
        assert!(!TOOLBAR.starts_with('«'));
        assert_eq!(TOOLBAR, "clear completed");
        assert!(!TOOLBAR.contains("sweep"));
        assert!(!TOOLBAR.contains("file"));
    }

    /// Round-2 item 2: the brackets are always normal text; only the
    /// state-char between them carries the state's color-coding from
    /// `theme::item_glyph`.
    #[test]
    fn checkbox_char_is_colored_but_brackets_are_normal_text() {
        let app = app_with(SAMPLE);
        let row = Row::Item {
            key: ItemKey {
                title: "X".into(),
                state: State::InProgress,
            },
            agent: None,
        };
        let line = row_line(&app, &row, 40);
        assert_eq!(line.spans[0].content, "[");
        assert_eq!(line.spans[0].style, theme::normal_text());
        assert_eq!(line.spans[1].content, "~");
        assert_eq!(line.spans[1].style, theme::item_glyph(State::InProgress));
        assert_eq!(line.spans[2].content, "] ");
        assert_eq!(line.spans[2].style, theme::normal_text());
    }

    /// A live session whose status herdr reports as Unknown must still read as
    /// live: without this it renders exactly like a claim whose agent is gone,
    /// which is the one distinction the sub-line exists to make.
    /// The bug this fixes: a long body line scrolled sideways behind a
    /// horizontal scrollbar instead of wrapping. Asserts the RENDERED buffer,
    /// not the widget's configuration — the socket regression taught that a
    /// setting can look right while the surface stays wrong.
    #[test]
    fn a_long_body_line_renders_across_several_wrapped_rows() {
        let item = crate::feed::Item {
            state: State::Open,
            title: "T".into(),
            agent: None,
            done_date: None,
            body: vec![
                "the quick brown fox jumps over the lazy dog and keeps running well past \
                 the right edge of this field"
                    .into(),
            ],
        };
        let key = crate::tui::app::ItemKey {
            title: "T".into(),
            state: State::Open,
        };
        let mut m = modal::EditModal::edit(key, &item, None, vec![]);

        let (w, h) = (60, 24);
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw_edit_modal(f, &mut m)).unwrap();
        let buf = term.backend().buffer().clone();
        let layout = modal::edit_layout(Rect::new(0, 0, w, h), true, false, m.dropdown_rows());
        let inner = Block::bordered().inner(layout.body);

        let rows: Vec<String> = (inner.y..inner.y + inner.height)
            .map(|y| {
                (inner.x..inner.x + inner.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        let filled: Vec<&String> = rows.iter().filter(|r| !r.is_empty()).collect();

        assert!(
            filled.len() >= 2,
            "a line wider than the field must occupy several rows, got {rows:?}"
        );
        assert!(
            filled[0].contains("the quick brown fox"),
            "first row should start the line, got {:?}",
            filled[0]
        );
        assert!(
            filled.iter().any(|r| r.contains("running")
                || r.contains("past")
                || r.contains("edge")),
            "later text must appear on a following row, not scrolled away: {rows:?}"
        );
    }

    #[test]
    fn a_live_agent_with_unknown_status_still_shows_a_live_marker() {
        let mut app = app_with(SAMPLE);
        app.statuses.insert(
            "0198f3ab".to_string(),
            AgentInfo {
                pane_id: "w1:p1".into(),
                kind: "claude".into(),
                session_id: "0198f3ab".into(),
                status: AgentStatus::Unknown,
            },
        );
        let rows = render_to_strings(&mut app, 20, 12);
        assert_eq!(rows[3], "  @claude ·");
    }

    #[test]
    fn agent_glyph_hidden_when_socket_absent() {
        let mut app = app_with(SAMPLE); // statuses empty
        let rows = render_to_strings(&mut app, 20, 12);
        assert_eq!(rows[3], "  @claude"); // sub-line still shown, no glyph
    }

    /// The `»` restore chevron always renders on the bottom row (matching
    /// native herdr), regardless of what status indicators sit above it.
    #[test]
    fn collapsed_rail_chevron_pinned_to_bottom_row() {
        let mut app = app_with(SAMPLE);
        app.collapsed = true;
        let rows = render_to_strings(&mut app, 3, 6);
        assert_eq!(rows[rows.len() - 1], "»");
    }

    /// Round-5 (Plan 3c): with reviews present and no blocked agent, the
    /// rail shows (top to bottom) the open-task count, a blank, the mauve
    /// `?` review indicator, blank padding, then the chevron. SAMPLE has 2
    /// open/in-progress items and 1 `[?]` item; no agent status is set, so
    /// no `!` appears.
    #[test]
    fn collapsed_rail_shows_open_count_and_review_indicator() {
        let mut app = app_with(SAMPLE);
        app.collapsed = true;
        let rows = render_to_strings(&mut app, 3, 6);
        assert_eq!(rows[0], "2", "open-task count"); // Fix auth + Migrate CI
        assert_eq!(rows[1], "", "blank separator");
        assert_eq!(rows[2], "?", "review indicator");
        assert_eq!(rows[3], "1", "review count");
        assert_eq!(rows[5], "»");
        assert!(!rows.contains(&"!".to_string()), "no blocked agent set");
    }

    /// No `[?]` items and no blocked agent: only the open count and the
    /// chevron render — the review and blocked indicators are omitted
    /// outright, not blanked-but-reserved.
    #[test]
    fn collapsed_rail_omits_review_and_blocked_indicators_when_absent() {
        let mut app = app_with("# Feed\n\n- [ ] A\n- [~] B\n");
        app.collapsed = true;
        let rows = render_to_strings(&mut app, 3, 6);
        assert_eq!(rows[0], "2");
        assert_eq!(rows[rows.len() - 1], "»");
        assert!(!rows.contains(&"?".to_string()));
        assert!(!rows.contains(&"!".to_string()));
    }

    /// A linked agent reporting Blocked shows the red `!` indicator.
    #[test]
    fn collapsed_rail_shows_blocked_indicator_for_linked_blocked_agent() {
        let mut app = app_with("- [~] Beta @agent(claude:abc)\n");
        app.statuses.insert(
            "abc".into(),
            AgentInfo {
                pane_id: "w1:p7".into(),
                kind: "claude".into(),
                session_id: "abc".into(),
                status: AgentStatus::Blocked,
            },
        );
        app.collapsed = true;
        let rows = render_to_strings(&mut app, 3, 6);
        assert!(rows.contains(&"!".to_string()), "got: {rows:?}");
        assert_eq!(rows[rows.len() - 1], "»");
    }

    /// Both the review and blocked indicators present together, in the
    /// documented top-to-bottom order: open count, blank, `?` + count,
    /// blank, `!`, then the chevron.
    #[test]
    fn collapsed_rail_shows_both_review_and_blocked_indicators_together() {
        let mut app = app_with("- [ ] A @agent(claude:abc)\n- [?] B @agent(claude:abc)\n- [ ] C\n");
        app.statuses.insert(
            "abc".into(),
            AgentInfo {
                pane_id: "w1:p7".into(),
                kind: "claude".into(),
                session_id: "abc".into(),
                status: AgentStatus::Blocked,
            },
        );
        app.collapsed = true;
        let rows = render_to_strings(&mut app, 3, 9);
        assert_eq!(rows[0], "2", "open count: A + C"); // B is [?], excluded
        assert_eq!(rows[1], "");
        assert_eq!(rows[2], "?");
        assert_eq!(rows[3], "1");
        assert_eq!(rows[4], "");
        assert_eq!(rows[5], "!");
        assert_eq!(rows[rows.len() - 1], "»");
    }

    /// Multi-digit counts stack one digit per row (spec example: "1","2" for
    /// 12), not a single "12" cell.
    #[test]
    fn collapsed_rail_stacks_multi_digit_open_count() {
        let items: String = (0..12).map(|i| format!("- [ ] Item {i}\n")).collect();
        let mut app = app_with(&items);
        app.collapsed = true;
        let rows = render_to_strings(&mut app, 3, 14);
        assert_eq!(rows[0], "1");
        assert_eq!(rows[1], "2");
    }

    /// The open-task count renders in normal text; the review indicator (and
    /// its count) in mauve; the blocked indicator in red — reusing the same
    /// theme helpers the expanded list already uses for these states/statuses.
    #[test]
    fn collapsed_rail_indicators_use_themed_colors() {
        let mut app = app_with("- [ ] A @agent(claude:abc)\n- [?] B @agent(claude:abc)\n");
        app.statuses.insert(
            "abc".into(),
            AgentInfo {
                pane_id: "w1:p7".into(),
                kind: "claude".into(),
                session_id: "abc".into(),
                status: AgentStatus::Blocked,
            },
        );
        app.collapsed = true;
        let buf = render_buffer(&mut app, 3, 8);
        assert_eq!(buf[(0, 0)].fg, theme::TEXT, "open count: normal text");
        assert_eq!(buf[(0, 2)].fg, theme::MAUVE, "review indicator: mauve");
        assert_eq!(buf[(0, 5)].fg, theme::RED, "blocked indicator: red");
    }

    #[test]
    fn list_scrolls() {
        let mut app = app_with(SAMPLE);
        app.scroll = 2;
        let rows = render_to_strings(&mut app, 20, 12);
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
    fn cursor_hidden_when_no_modal_active() {
        let mut app = app_with(SAMPLE);
        assert_eq!(render_cursor(&mut app, 40, 12), None);
    }

    /// Cursor fix: the real terminal cursor is never placed for the edit
    /// modal any more — the focused textarea renders its own viewport-correct
    /// cursor cell instead (`sync_blocks`). `render_cursor` (which reports
    /// the real terminal cursor) must stay `None` even while a field is
    /// focused; the visible cursor is a REVERSED buffer cell at the focused
    /// field's (viewport-corrected) cursor position.
    ///
    /// Task 9: Category is now drawn (previously only Title/Body were), so
    /// this checks the reversed cell lands there when it's the focused field
    /// — create mode opens with Category already focused (spec
    /// 2026-09-16 §2).
    #[test]
    fn reversed_cursor_cell_at_category_when_create_modal_opens() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenCreate);
        assert_eq!(
            render_cursor(&mut app, 60, 20),
            None,
            "no real terminal cursor is ever placed any more"
        );
        let buf = render_buffer(&mut app, 60, 20);
        let layout = modal::edit_layout(Rect::new(0, 0, 60, 20), false, false, 0);
        let inner = Block::bordered().inner(layout.category);
        assert!(
            buf[(inner.x, inner.y)]
                .modifier
                .contains(Modifier::REVERSED),
            "focused Category field must show tui-textarea's own reversed cursor cell"
        );
    }

    #[test]
    fn reversed_cursor_cell_at_title_after_tab_from_category() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenCreate);
        // Category is focused first (spec 2026-09-16 §2); Tab past it.
        app.handle_modal_event(
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            (60, 20),
        );
        assert_eq!(render_cursor(&mut app, 60, 20), None);
        let buf = render_buffer(&mut app, 60, 20);
        let layout = modal::edit_layout(Rect::new(0, 0, 60, 20), false, false, 0);
        // Untouched textarea: cursor still at its origin (row 0, col 0).
        let title_inner = Block::bordered().inner(layout.title);
        assert!(
            buf[(title_inner.x, title_inner.y)]
                .modifier
                .contains(Modifier::REVERSED),
            "focused Title field must show the reversed cursor cell"
        );
        // Category lost focus — its own cursor cell must go invisible again.
        let category_inner = Block::bordered().inner(layout.category);
        assert!(
            !buf[(category_inner.x, category_inner.y)]
                .modifier
                .contains(Modifier::REVERSED),
            "unfocused Category must not show a reversed cursor cell"
        );
    }

    /// Round-2 item 5: the "Add to: <section>" picker row is gone entirely —
    /// sidebar-created items always land in the first human section.
    /// A button focus (e.g. Save) shows no reversed cursor cell anywhere —
    /// none of the three textareas are focused, so `sync_blocks` leaves all
    /// three with the invisible cursor style.
    #[test]
    fn cursor_hidden_when_focus_on_save_button() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenCreate);
        // Category -> Title -> Body -> Save.
        for _ in 0..3 {
            app.handle_modal_event(
                Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
                (40, 12),
            );
        }
        let Modal::Edit(m) = &app.modal else {
            panic!("expected edit modal")
        };
        assert_eq!(m.focus, EditFocus::Save);
        assert_eq!(
            render_cursor(&mut app, 40, 12),
            None,
            "no real terminal cursor when a button is focused"
        );
        let buf = render_buffer(&mut app, 40, 12);
        let layout = modal::edit_layout(Rect::new(0, 0, 40, 12), false, false, 0);
        for field in [layout.category, layout.title, layout.body] {
            let inner = Block::bordered().inner(field);
            assert!(
                !buf[(inner.x, inner.y)]
                    .modifier
                    .contains(Modifier::REVERSED),
                "no field's cursor cell should be reversed when a button is focused"
            );
        }
    }

    /// Regression test for the actual bug: tui-textarea 0.7 exposes no
    /// viewport getter, so placing the REAL terminal cursor from the LOGICAL
    /// cursor column drifted from the drawn text whenever the textarea had
    /// scrolled horizontally (a body line longer than the field's width).
    /// Rendering the focused field's OWN cursor cell sidesteps this — it is
    /// always wherever tui-textarea actually drew the cursor, viewport scroll
    /// included. Render a narrow terminal with a body line far longer than
    /// the field width, focus on Body, and confirm the reversed cell lands
    /// inside the body field's inner rect (the old logical-column code could
    /// place the terminal cursor off-rect or past the visible text here).
    #[test]
    fn reversed_cursor_cell_stays_inside_body_when_line_exceeds_field_width() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenCreate);
        let (w, h) = (30, 20); // narrow — the herdr docked-sidebar width
        app.handle_modal_event(
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            (w, h),
        );
        app.handle_modal_event(
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            (w, h),
        );
        let Modal::Edit(m) = &mut app.modal else {
            panic!("expected edit modal")
        };
        assert_eq!(m.focus, EditFocus::Body);
        let long_line = "a".repeat(200);
        m.body.insert_str(&long_line);

        let buf = render_buffer(&mut app, w, h);
        let layout = modal::edit_layout(Rect::new(0, 0, w, h), false, false, 0);
        let inner = Block::bordered().inner(layout.body);

        let mut found = None;
        for y in inner.y..inner.y + inner.height {
            for x in inner.x..inner.x + inner.width {
                if buf[(x, y)].modifier.contains(Modifier::REVERSED) {
                    found = Some((x, y));
                }
            }
        }
        let (x, y) = found.expect("a reversed cursor cell must render inside the body field");
        assert!(
            x >= inner.x && x < inner.x + inner.width && y >= inner.y && y < inner.y + inner.height,
            "reversed cell ({x},{y}) must fall within body's inner rect {inner:?}"
        );
    }

    #[test]
    fn create_modal_shows_save_cancel_but_no_delete_or_section_picker() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenCreate);
        let rows = render_to_strings(&mut app, 60, 20);
        let joined = rows.join("\n");
        assert!(!joined.contains("Add to"), "got:\n{joined}");
        assert!(joined.contains(modal::SAVE_LABEL), "got:\n{joined}");
        assert!(joined.contains(modal::CANCEL_LABEL), "got:\n{joined}");
        assert!(!joined.contains(modal::DELETE_LABEL), "got:\n{joined}");
    }

    /// Crash regression (production panic): `feedr::tui::run`'s draw loop
    /// panicked with "index outside of buffer" inside `Paragraph::render`
    /// when a task was clicked (opening the edit modal, which has a Delete
    /// button) while the sidebar pane was a narrow ~30 columns — the width
    /// herdr docks a sidebar pane at. Root cause: `modal::edit_layout`'s
    /// button row placed the Delete button's `Rect` past the right edge of
    /// the terminal itself (not just the modal panel) via unbounded
    /// x-cursor arithmetic; `render_button` then handed that Rect straight
    /// to `Paragraph::render`, which panics instead of clipping. This drives
    /// the exact failing path end-to-end through a real `TestBackend` draw
    /// at the reproducing size (30x40, matching the field crash) rather than
    /// just checking geometry, so a future regression anywhere in the render
    /// path — not only the layout math `edit_layout_buttons_never_escape_
    /// the_terminal_at_any_width` in modal.rs guards — is caught too.
    #[test]
    fn edit_modal_survives_narrow_pane_render_without_panicking() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenEdit(ItemKey {
            title: "Fix auth redirect loop".into(),
            state: State::Open,
        }));
        let rows = render_to_strings(&mut app, 30, 40);
        assert!(!rows.is_empty());
    }

    /// Task 9: the category field and its suggestion dropdown must actually
    /// render, not just exist in the layout geometry (Tasks 6-8 wired the
    /// geometry and state; this is the first test to draw them).
    #[test]
    fn edit_modal_draws_category_field_and_dropdown() {
        let mut m = modal::EditModal::create(vec!["Work".into(), "Chores".into()]);
        m.dropdown = Some(0); // open the dropdown so both field and list render
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw_edit_modal(f, &mut m)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = (0..24)
            .map(|y| {
                (0..80)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Category"), "category field must render");
        assert!(
            text.contains("Work") && text.contains("Chores"),
            "suggestions must render"
        );

        // Fix: the dropdown now draws its OWN bordered, tinted box — not
        // bare text floating over the field below it (which used to read as
        // form labels rather than a popup). Check both the border glyph
        // itself and its distinct accent color at the dropdown's own
        // corner, and the tinted background on a non-highlighted row.
        let layout = modal::edit_layout(Rect::new(0, 0, 80, 24), false, false, m.dropdown_rows());
        let corner = &buf[(layout.dropdown.x, layout.dropdown.y)];
        assert_eq!(
            corner.symbol(),
            "┌",
            "dropdown must draw its own top-left border corner"
        );
        assert_eq!(
            corner.fg,
            theme::LAVENDER,
            "dropdown border uses its own accent color, distinct from the modal's Surface1 border"
        );
        // Row 0 ("Work") is highlighted (m.dropdown = Some(0)); row 1
        // ("Chores") is the plain, non-highlighted tinted row.
        let plain_row = &buf[(layout.dropdown_inner.x, layout.dropdown_inner.y + 1)];
        assert_eq!(
            plain_row.bg,
            theme::SURFACE1,
            "non-highlighted suggestion rows are tinted so the popup reads as a distinct surface"
        );
    }

    /// Rendering regression for the reported bug: opening an existing item
    /// whose body's first line is long (100+ chars) and whose body has
    /// 10+ lines used to show the field scrolled to the TAIL of that line
    /// (and the body scrolled to its bottom) on the modal's first frame at a
    /// narrow (pre-zoom) pane width, even though nothing was focused there.
    /// After parking cursors at start (`modal::EditModal::edit`), the first
    /// visible row of the body must show the LEADING characters of line 0,
    /// not its tail.
    #[test]
    fn edit_modal_opens_with_body_viewport_showing_line_start_not_tail() {
        let long_first_line = "A".repeat(120);
        let mut body = vec![long_first_line];
        for i in 1..12 {
            body.push(format!("line {i}"));
        }
        let item = crate::feed::Item {
            state: State::Open,
            title: "T".into(),
            agent: None,
            done_date: None,
            body,
        };
        let key = ItemKey {
            title: "T".into(),
            state: State::Open,
        };
        let mut m = modal::EditModal::edit(key, &item, None, vec![]);

        let (w, h) = (30, 24); // narrow — the herdr docked-sidebar width
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw_edit_modal(f, &mut m)).unwrap();
        let buf = terminal.backend().buffer().clone();

        let layout = modal::edit_layout(Rect::new(0, 0, w, h), true, false, 0);
        let inner = Block::bordered().inner(layout.body);
        let first_row: String = (inner.x..inner.x + inner.width)
            .map(|x| buf[(x, inner.y)].symbol().to_string())
            .collect();
        assert!(
            first_row.trim_start().starts_with('A'),
            "body's first visible row must show the START of line 0, got {first_row:?}"
        );
    }

    /// The sticky-scroll regression: the modal's FIRST frame renders at the
    /// narrow pre-zoom pane width, which scrolls the focused field's
    /// viewport right (its cursor parks at the end); tui-textarea never
    /// scrolls back on its own once the pane widens. The per-frame viewport
    /// reset in `draw_edit_modal` must make the NEXT (wide) frame render
    /// the title from its start again — the scroll is recomputed for the
    /// current size, not inherited from the narrow frame.
    #[test]
    fn wide_frame_after_narrow_frame_shows_focused_title_from_start() {
        let item = crate::feed::Item {
            state: State::Open,
            title: "A title long enough to overflow a narrow docked pane".into(),
            agent: None,
            done_date: None,
            body: Vec::new(),
        };
        let key = ItemKey {
            title: item.title.clone(),
            state: State::Open,
        };
        // Categorized → Title focused, cursor parked at its END.
        let mut m = modal::EditModal::edit(key, &item, Some("Work".into()), vec![]);

        // Frame 1: narrow — the title scrolls to keep the end-cursor visible.
        let mut narrow =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 24)).unwrap();
        narrow.draw(|f| draw_edit_modal(f, &mut m)).unwrap();

        // Frame 2: wide — the title fits; it must render from its start.
        let (w, h) = (100, 24);
        let mut wide = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        wide.draw(|f| draw_edit_modal(f, &mut m)).unwrap();
        let buf = wide.backend().buffer().clone();
        let layout = modal::edit_layout(Rect::new(0, 0, w, h), true, false, m.dropdown_rows());
        let inner = Block::bordered().inner(layout.title);
        let row: String = (inner.x..inner.x + inner.width)
            .map(|x| buf[(x, inner.y)].symbol().to_string())
            .collect();
        assert!(
            row.trim_start().starts_with("A title long"),
            "wide frame must not inherit the narrow frame's scroll, got {row:?}"
        );
        // And the per-frame viewport reset must NOT have dragged the cursor
        // along (tui-textarea's `scroll()` clamps the cursor into the
        // scrolled viewport — verified live: typing after a draw landed at
        // the START of the field until the cursor restore was added).
        assert_eq!(
            m.title.cursor(),
            (0, item.title.len()),
            "drawing must never move the cursor"
        );
    }

    #[test]
    fn edit_modal_shows_delete_button() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenEdit(ItemKey {
            title: "Fix auth redirect loop".into(),
            state: State::Open,
        }));
        let rows = render_to_strings(&mut app, 60, 20);
        let joined = rows.join("\n");
        assert!(joined.contains(modal::DELETE_LABEL), "got:\n{joined}");
    }

    // --- Round-3 item 1: dimmed-backdrop modal overlay ---------------------
    //
    // Every modal is a true overlay now: the whole frame is repainted as a
    // dimmed backdrop (Modifier::DIM + theme::BASE bg) before the panel is
    // drawn, and the panel itself gets a solid, undimmed background
    // (theme::MANTLE) with a Surface1 border — so the panel never carries
    // the backdrop's dim, and everything drawn inside it (buttons,
    // textareas) inherits the panel's own background.

    #[test]
    fn edit_modal_overlay_dims_backdrop_paints_panel_and_propagates_bg_to_buttons() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenCreate);
        let (w, h) = (60, 20);
        let buf = render_buffer(&mut app, w, h);
        let layout = modal::edit_layout(Rect::new(0, 0, w, h), false, false, 0);

        // Top-left corner is outside the centered 90%x80% panel.
        let outside = &buf[(0, 0)];
        assert!(
            outside.modifier.contains(Modifier::DIM),
            "cell outside the panel must be dimmed"
        );
        assert_eq!(outside.bg, theme::BASE);

        let border_cell = &buf[(layout.outer.x, layout.outer.y)];
        assert!(
            !border_cell.modifier.contains(Modifier::DIM),
            "panel border must not carry the backdrop's dim"
        );
        assert_eq!(border_cell.fg, theme::SURFACE1);
        assert_eq!(border_cell.bg, theme::MANTLE);

        let save_cell = &buf[(layout.save.x, layout.save.y)];
        assert!(
            !save_cell.modifier.contains(Modifier::DIM),
            "buttons must not carry the backdrop's dim"
        );
        assert_eq!(
            save_cell.bg,
            theme::MANTLE,
            "buttons must inherit the panel's own background"
        );
    }

    #[test]
    fn confirm_delete_modal_dims_backdrop_and_paints_its_own_panel() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenEdit(ItemKey {
            title: "Fix auth redirect loop".into(),
            state: State::Open,
        }));
        let (w, h) = (60, 20);
        app.handle_modal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            (w, h),
        );
        assert!(matches!(app.modal, Modal::ConfirmDelete(_)));

        let buf = render_buffer(&mut app, w, h);
        let outer = modal::centered(Rect::new(0, 0, w, h), 80, 20);

        assert!(buf[(0, 0)].modifier.contains(Modifier::DIM));
        let border_cell = &buf[(outer.x, outer.y)];
        assert!(!border_cell.modifier.contains(Modifier::DIM));
        assert_eq!(border_cell.fg, theme::SURFACE1);
        assert_eq!(border_cell.bg, theme::MANTLE);
    }

    #[test]
    fn done_view_modal_dims_backdrop_and_paints_its_own_panel() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenDoneView);
        let (w, h) = (60, 20);
        let buf = render_buffer(&mut app, w, h);
        let outer = modal::centered(Rect::new(0, 0, w, h), 90, 80);

        assert!(buf[(0, 0)].modifier.contains(Modifier::DIM));
        let border_cell = &buf[(outer.x, outer.y)];
        assert!(!border_cell.modifier.contains(Modifier::DIM));
        assert_eq!(border_cell.fg, theme::SURFACE1);
        assert_eq!(border_cell.bg, theme::MANTLE);
    }

    #[test]
    fn file_view_modal_dims_backdrop_and_paints_its_own_panel() {
        let mut app = app_with(SAMPLE);
        app.apply(Action::OpenFileView);
        let (w, h) = (60, 20);
        let buf = render_buffer(&mut app, w, h);
        let outer = modal::centered(Rect::new(0, 0, w, h), 90, 80);

        assert!(buf[(0, 0)].modifier.contains(Modifier::DIM));
        let border_cell = &buf[(outer.x, outer.y)];
        assert!(!border_cell.modifier.contains(Modifier::DIM));
        assert_eq!(border_cell.fg, theme::SURFACE1);
        assert_eq!(border_cell.bg, theme::MANTLE);
    }

    #[test]
    fn no_dim_applied_when_no_modal_active() {
        let mut app = app_with(SAMPLE);
        let buf = render_buffer(&mut app, 60, 20);
        assert!(!buf[(0, 0)].modifier.contains(Modifier::DIM));
    }
}
