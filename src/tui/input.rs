use crate::tui::app::{Action, App, Row};
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};

/// Pure event → action mapping. `size` is the terminal (width, height);
/// geometry must mirror view::draw: y=0 toolbar, list at y=1..=h-3,
/// Done(n) at h-2, status line at h-1.
pub fn translate(ev: &Event, app: &App, size: (u16, u16)) -> Option<Action> {
    let (w, h) = size;
    match ev {
        Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('a') => Some(Action::OpenCreate),
            KeyCode::Char('e') => Some(Action::OpenEditor),
            KeyCode::Char('f') => Some(Action::OpenFileView),
            KeyCode::Up => Some(Action::ScrollUp),
            KeyCode::Down => Some(Action::ScrollDown),
            _ => None,
        },
        Event::Mouse(m) => match m.kind {
            MouseEventKind::ScrollUp => Some(Action::ScrollUp),
            MouseEventKind::ScrollDown => Some(Action::ScrollDown),
            MouseEventKind::Down(MouseButton::Left) => click(app, m.column, m.row, w, h),
            _ => None,
        },
        _ => None,
    }
}

/// Width, in cells, of the bottom-right collapse-chevron click zone on the
/// status row — must match `view::CHEVRON_ZONE_WIDTH` so the clickable area
/// and the drawn glyph never drift apart.
const CHEVRON_ZONE_WIDTH: u16 = 2;

fn click(app: &App, x: u16, y: u16, w: u16, h: u16) -> Option<Action> {
    if app.collapsed {
        // Any click on the rail restores the sidebar.
        return Some(Action::ToggleCollapse);
    }
    if y == 0 {
        // Toolbar columns must match view::TOOLBAR ("clear completed").
        // Round-4 item 1: "file" is gone from the toolbar — the file
        // viewer is reachable via the `f` key instead — so everything
        // right of "clear completed" is a dead zone.
        return match x {
            0..=14 => Some(Action::Sweep),
            _ => None,
        };
    }
    if h >= 1 && y == h - 1 {
        // Status row: the collapse chevron owns the rightmost
        // CHEVRON_ZONE_WIDTH cells (round-3 item 2, native-herdr position);
        // the rest of the row is the (unclickable) status message.
        let chevron_w = CHEVRON_ZONE_WIDTH.min(w);
        return (chevron_w > 0 && x >= w - chevron_w).then_some(Action::ToggleCollapse);
    }
    if h >= 2 && y == h - 2 {
        return Some(Action::OpenDoneView);
    }
    if h >= 4 && (1..=h - 3).contains(&y) {
        let idx = app.scroll + (y as usize - 1);
        return match app.rows.get(idx)? {
            Row::Section(_) => None,
            // Row is "[<char>] title" (round-2 item 2): checkbox zone is
            // x 0..=2 (the bracket pair + state char), title starts at x 4;
            // x==3 (the space after "]") is a dead zone between them.
            Row::Item { key, .. } => {
                if x <= 2 {
                    Some(Action::Advance(key.clone()))
                } else if x >= 4 {
                    Some(Action::OpenEdit(key.clone()))
                } else {
                    None
                }
            }
            Row::AgentLine { key, .. } => Some(Action::AgentClick(key.clone())),
            Row::Add => Some(Action::OpenCreate),
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Side, SidebarConfig};
    use crate::feed::parse::parse;
    use crate::feed::State;
    use crate::tui::app::{Action, App, ItemKey};
    use crate::tui::socket::FakeHerdr;
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    fn app_with(text: &str) -> App {
        let cfg = SidebarConfig {
            side: Side::Left,
            width: 0.18,
            max_width: 46,
            auto_dock: false,
        };
        let mut app = App::new(
            "/nonexistent/feed.md".into(),
            cfg,
            Box::new(FakeHerdr::default()),
        );
        app.doc = parse(text);
        app.rebuild();
        app
    }

    const SAMPLE: &str = "\
# Feed

- [ ] Alpha
- [?] Beta @agent(claude:abc)

# Done

## Feed

- [x] Old @done(2026-09-01)
";
    // Rows: 0 I:Alpha · 1 I:Beta · 2 A:claude · 3 Add

    fn click(x: u16, y: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    const SIZE: (u16, u16) = (22, 10); // list rows y=1..=7, Done row y=8, status y=9; w=22 so the toolbar ("clear completed") and the bottom-right chevron zone both fit

    #[test]
    fn keys_quit_add_editor_scroll() {
        let app = app_with(SAMPLE);
        assert_eq!(translate(&key('q'), &app, SIZE), Some(Action::Quit));
        assert_eq!(translate(&key('a'), &app, SIZE), Some(Action::OpenCreate));
        assert_eq!(translate(&key('e'), &app, SIZE), Some(Action::OpenEditor));
        assert_eq!(
            translate(
                &Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
                &app,
                SIZE
            ),
            Some(Action::ScrollDown)
        );
    }

    /// Round-3 item 3: toolbar is now "clear completed" — no leading
    /// chevron (that moved to the status row, round-3 item 2). Round-4
    /// item 1: "file" is gone from the toolbar entirely — "clear completed"
    /// still spans 0..=14, and everything right of it is a dead zone. The
    /// file viewer moved to the `f` key (see `f_key_opens_file_view`).
    #[test]
    fn toolbar_clicks() {
        let app = app_with(SAMPLE);
        assert_eq!(translate(&click(0, 0), &app, SIZE), Some(Action::Sweep));
        assert_eq!(translate(&click(14, 0), &app, SIZE), Some(Action::Sweep));
        assert_eq!(translate(&click(15, 0), &app, SIZE), None); // dead zone
        assert_eq!(translate(&click(17, 0), &app, SIZE), None); // dead zone (was "file")
        assert_eq!(translate(&click(21, 0), &app, SIZE), None); // dead zone
    }

    /// Round-4 item 1: with "file" removed from the toolbar, the internal
    /// file viewer stays reachable via the `f` key, alongside the existing
    /// `e` → $EDITOR binding.
    #[test]
    fn f_key_opens_file_view() {
        let app = app_with(SAMPLE);
        assert_eq!(translate(&key('f'), &app, SIZE), Some(Action::OpenFileView));
    }

    /// Round-3 item 2: the collapse chevron moved off the toolbar onto the
    /// bottom-right of the status row (native-herdr position), owning the
    /// rightmost 2 cells of that row; the rest of the status row (where the
    /// status message renders) is a dead zone.
    #[test]
    fn status_row_chevron_click_toggles_collapse() {
        let app = app_with(SAMPLE);
        // SIZE = (22, 10): status row y = h-1 = 9; chevron zone x = 20..=21.
        assert_eq!(
            translate(&click(21, 9), &app, SIZE),
            Some(Action::ToggleCollapse)
        );
        assert_eq!(
            translate(&click(20, 9), &app, SIZE),
            Some(Action::ToggleCollapse)
        );
        assert_eq!(translate(&click(19, 9), &app, SIZE), None); // status text zone
        assert_eq!(translate(&click(0, 9), &app, SIZE), None); // status text zone
    }

    /// Round-2 item 2: row is "[<char>] title" — checkbox zone x 0..=2,
    /// title starts at x 4, x==3 is a dead zone.
    #[test]
    fn checkbox_click_vs_title_click() {
        let app = app_with(SAMPLE);
        let alpha = ItemKey {
            title: "Alpha".into(),
            state: State::Open,
        };
        for x in 0..=2 {
            assert_eq!(
                translate(&click(x, 1), &app, SIZE),
                Some(Action::Advance(alpha.clone())),
                "x={x}"
            );
        }
        assert_eq!(translate(&click(3, 1), &app, SIZE), None); // dead zone
        assert_eq!(
            translate(&click(4, 1), &app, SIZE),
            Some(Action::OpenEdit(alpha.clone()))
        );
        assert_eq!(
            translate(&click(7, 1), &app, SIZE),
            Some(Action::OpenEdit(alpha))
        );
    }

    #[test]
    fn agent_line_add_row_and_done_row_clicks() {
        let app = app_with(SAMPLE);
        let beta = ItemKey {
            title: "Beta".into(),
            state: State::Review,
        };
        assert_eq!(
            translate(&click(4, 3), &app, SIZE),
            Some(Action::AgentClick(beta))
        );
        assert_eq!(
            translate(&click(2, 4), &app, SIZE),
            Some(Action::OpenCreate)
        ); // + add
        assert_eq!(
            translate(&click(3, 8), &app, SIZE),
            Some(Action::OpenDoneView)
        );
        assert_eq!(translate(&click(3, 6), &app, SIZE), None); // empty list space
    }

    #[test]
    fn scroll_offsets_hit_testing_and_collapsed_restores() {
        let mut app = app_with(SAMPLE);
        app.scroll = 2;
        // y=1 now hits row 2 (the agent line):
        let beta = ItemKey {
            title: "Beta".into(),
            state: State::Review,
        };
        assert_eq!(
            translate(&click(4, 1), &app, SIZE),
            Some(Action::AgentClick(beta))
        );
        app.collapsed = true;
        assert_eq!(
            translate(&click(1, 4), &app, SIZE),
            Some(Action::ToggleCollapse)
        );
    }
}
