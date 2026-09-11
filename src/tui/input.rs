use crate::tui::app::{Action, App, Row};
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};

/// Pure event → action mapping. `size` is the terminal (width, height);
/// geometry must mirror view::draw: y=0 toolbar, list at y=1..=h-3,
/// Done(n) at h-2, status line at h-1.
pub fn translate(ev: &Event, app: &App, size: (u16, u16)) -> Option<Action> {
    let (_w, h) = size;
    match ev {
        Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('a') => Some(Action::OpenCreate),
            KeyCode::Char('e') => Some(Action::OpenEditor),
            KeyCode::Up => Some(Action::ScrollUp),
            KeyCode::Down => Some(Action::ScrollDown),
            _ => None,
        },
        Event::Mouse(m) => match m.kind {
            MouseEventKind::ScrollUp => Some(Action::ScrollUp),
            MouseEventKind::ScrollDown => Some(Action::ScrollDown),
            MouseEventKind::Down(MouseButton::Left) => click(app, m.column, m.row, h),
            _ => None,
        },
        _ => None,
    }
}

fn click(app: &App, x: u16, y: u16, h: u16) -> Option<Action> {
    if app.collapsed {
        // Any click on the rail restores the sidebar.
        return Some(Action::ToggleCollapse);
    }
    if y == 0 {
        // Toolbar columns must match view::TOOLBAR ("« sweep file").
        return match x {
            0 => Some(Action::ToggleCollapse),
            2..=6 => Some(Action::Sweep),
            8..=11 => Some(Action::OpenFileView),
            _ => None,
        };
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

    const SIZE: (u16, u16) = (20, 10); // list rows y=1..=7, Done row y=8, status y=9

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

    #[test]
    fn toolbar_clicks() {
        let app = app_with(SAMPLE);
        assert_eq!(
            translate(&click(0, 0), &app, SIZE),
            Some(Action::ToggleCollapse)
        );
        assert_eq!(translate(&click(4, 0), &app, SIZE), Some(Action::Sweep));
        assert_eq!(
            translate(&click(9, 0), &app, SIZE),
            Some(Action::OpenFileView)
        );
        assert_eq!(translate(&click(15, 0), &app, SIZE), None); // dead zone
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
