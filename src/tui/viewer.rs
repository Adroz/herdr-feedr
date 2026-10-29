//! Read-only text viewers: the Done archive and the feed file view.
//!
//! A viewer is drawn through a read-only `TextArea` (never fed typing), so
//! it gets the edit fields' selection model for free: clicks and drags go
//! through `selection`, which maps the pointer against the layout as last
//! drawn (wrap and scroll included).

use crate::tui::{selection, theme};
use crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Style;
use tui_textarea::{CursorMove, TextArea, WrapMode};

pub struct Viewer {
    pub text: TextArea<'static>,
    /// The text `text` was last built from; `sync` rebuilds only on change.
    source: Option<String>,
    /// Text copied by a drag release or Ctrl+C, waiting for `App` to push it
    /// to the system clipboard (same one-shot as `EditModal`'s).
    pub pending_clipboard: Option<String>,
}

impl Default for Viewer {
    fn default() -> Self {
        let mut v = Viewer {
            text: TextArea::default(),
            source: None,
            pending_clipboard: None,
        };
        v.style();
        v
    }
}

impl Viewer {
    /// Show `text`. A no-op while it's unchanged, so the selection and scroll
    /// survive redraws; a change (the feed was edited underneath) rebuilds
    /// the text and keeps the cursor, and with it the scroll, where it was.
    pub fn sync(&mut self, text: &str) {
        if self.source.as_deref() == Some(text) {
            return;
        }
        // `str::lines` splits on LF and CRLF alike, so a CRLF feed shows no
        // stray `\r` (and copies none).
        let (row, col) = self.text.cursor();
        self.text = TextArea::from(text.lines());
        self.style();
        let clamp = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
        self.text
            .move_cursor(CursorMove::Jump(clamp(row), clamp(col)));
        self.source = Some(text.to_string());
    }

    fn style(&mut self) {
        let ta = &mut self.text;
        ta.set_wrap_mode(WrapMode::Word);
        ta.set_style(theme::normal_text());
        ta.set_selection_style(theme::selection());
        // Read-only: no visible cursor, no underlined cursor line.
        ta.set_cursor_style(Style::default());
        ta.set_cursor_line_style(Style::default());
    }

    /// Selection, copy and scroll input. Closing (Esc, `q`, outside click)
    /// and the file view's `e` are the caller's.
    pub fn handle(&mut self, ev: &Event) {
        match ev {
            Event::Key(k)
                if k.code == KeyCode::Char('c') && k.modifiers == KeyModifiers::CONTROL =>
            {
                self.pending_clipboard = selection::copy(&mut self.text);
            }
            Event::Key(k) if k.code == KeyCode::Down => self.scroll(1),
            Event::Key(k) if k.code == KeyCode::Up => self.scroll(-1),
            Event::Mouse(m) => match m.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    selection::press(&mut self.text, m.column, m.row);
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    selection::drag_to(&mut self.text, m.column, m.row);
                }
                // Copy-on-select, as in the edit fields.
                MouseEventKind::Up(MouseButton::Left) => {
                    if let Some(text) = selection::copy(&mut self.text) {
                        self.pending_clipboard = Some(text);
                    }
                }
                MouseEventKind::ScrollDown => self.scroll(1),
                MouseEventKind::ScrollUp => self.scroll(-1),
                _ => {}
            },
            _ => {}
        }
    }

    /// Scroll by `rows` drawn rows. Drops the selection first: tui-textarea
    /// scrolls by moving the cursor, which would otherwise extend it.
    fn scroll(&mut self, rows: i16) {
        self.text.cancel_selection();
        self.text.scroll((rows, 0));
    }
}

/// Select and copy in the read-only viewers (#35), driven like a user:
/// open a viewer, find the drawn text on screen, drag over it.
#[cfg(test)]
mod tests {
    use crate::tui::driver::Driver;
    use crate::tui::AppEvent;
    use crossterm::event::{KeyCode, KeyModifiers};

    const W: u16 = 60;
    const H: u16 = 24;

    fn open_done(feed: &str) -> Driver {
        let mut d = Driver::new(feed, W, H);
        d.click(0, H - 3); // the Done(n) row
        assert!(shows(&d, "Done archive"), "{}", d.screen().join("\n"));
        d
    }

    fn shows(d: &Driver, text: &str) -> bool {
        d.screen().iter().any(|l| l.contains(text))
    }

    /// The cell where `needle` starts on screen (rows of single-width chars,
    /// box-drawing borders included).
    fn find(d: &Driver, needle: &str) -> (u16, u16) {
        d.screen()
            .iter()
            .enumerate()
            .find_map(|(y, l)| {
                l.find(needle)
                    .map(|b| (l[..b].chars().count() as u16, y as u16))
            })
            .unwrap_or_else(|| panic!("{needle:?} not on screen:\n{}", d.screen().join("\n")))
    }

    fn status(d: &Driver) -> String {
        d.screen().last().unwrap().clone()
    }

    #[test]
    fn dragging_in_the_done_archive_copies_what_was_dragged_over() {
        let mut d =
            open_done("# Work\n\n- [ ] Alpha\n\n# Done\n\n- [x] Old thing @done(2026-01-01)\n");
        let (x, y) = find(&d, "Old thing");

        d.drag(&[(x, y), (x + 9, y)]);

        assert_eq!(d.clipboard_writes(), ["Old thing"]);
        assert_eq!(d.highlighted(), "Old thing");
        assert!(status(&d).contains("Copied 9 chars"), "{:#?}", d.screen());
    }

    fn open_file(feed: &str) -> Driver {
        let mut d = Driver::new(feed, W, H);
        d.key(KeyCode::Char('f'), KeyModifiers::NONE);
        assert!(shows(&d, "feed file"), "{}", d.screen().join("\n"));
        d
    }

    /// A feed far taller than the viewer: `- [ ] item00` .. `item59`.
    fn tall_feed() -> String {
        let items: String = (0..60).map(|i| format!("- [ ] item{i:02}\n")).collect();
        format!("# Feed\n\n{items}")
    }

    #[test]
    fn dragging_in_a_scrolled_file_view_copies_what_was_dragged_over() {
        let mut d = open_file(&tall_feed());
        for _ in 0..30 {
            d.key(KeyCode::Down, KeyModifiers::NONE);
        }
        assert!(!shows(&d, "- [ ] item00"), "Down scrolls the file view");
        let (x, y) = find(&d, "item40");

        // From "item40" down to the end of the next row's "item41".
        d.drag(&[(x, y), (x + 6, y + 1)]);

        assert_eq!(d.clipboard_writes(), ["item40\n- [ ] item41"]);
        // The selected line break draws as one highlighted cell.
        assert_eq!(d.highlighted(), "item40 \n- [ ] item41");
    }

    #[test]
    fn the_wheel_scrolls_a_viewer_too() {
        let mut d = open_file(&tall_feed());
        for _ in 0..30 {
            d.wheel_down(10, 10);
        }
        assert!(!shows(&d, "- [ ] item00"), "{}", d.screen().join("\n"));
        let (x, y) = find(&d, "item40");

        d.drag(&[(x, y), (x + 6, y)]);

        assert_eq!(d.clipboard_writes(), ["item40"]);
    }

    const DONE_FEED: &str =
        "# Work\n\n- [ ] Alpha\n\n# Done\n\n- [x] Old thing @done(2026-01-01)\n";

    #[test]
    fn ctrl_c_in_a_viewer_copies_the_selection_again() {
        let mut d = open_done(DONE_FEED);
        let (x, y) = find(&d, "Old thing");
        d.drag(&[(x, y), (x + 3, y)]);

        d.key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert_eq!(d.clipboard_writes(), ["Old", "Old"]);
        assert!(status(&d).contains("Copied 3 chars"), "{:#?}", d.screen());
        assert!(shows(&d, "Done archive"), "Ctrl+C keeps the viewer open");
    }

    #[test]
    fn esc_closes_a_viewer_even_with_text_selected() {
        let mut d = open_done(DONE_FEED);
        let (x, y) = find(&d, "Old thing");
        d.drag(&[(x, y), (x + 3, y)]);

        d.key(KeyCode::Esc, KeyModifiers::NONE);

        assert!(!shows(&d, "Done archive"), "{}", d.screen().join("\n"));
    }

    #[test]
    fn an_outside_click_still_closes_a_viewer_with_text_selected() {
        let mut d = open_file(DONE_FEED);
        let (x, y) = find(&d, "- [ ] Alpha"); // the viewer's, not the list's
        let x = x + 6;
        d.drag(&[(x, y), (x + 5, y)]);

        d.click(1, 0); // Alpha's checkbox, outside the panel

        assert!(!shows(&d, "feed file"), "{}", d.screen().join("\n"));
        assert_eq!(d.feed(), DONE_FEED, "the dismissing click is consumed");
    }

    #[test]
    fn a_crlf_feed_shows_and_copies_no_carriage_returns() {
        let feed = DONE_FEED.replace('\n', "\r\n");
        let mut d = open_file(&feed);
        assert!(
            !d.screen().iter().any(|l| l.contains('\r')),
            "{:#?}",
            d.screen()
        );
        let (x, y) = find(&d, "- [ ] Alpha"); // the viewer's, not the list's
        let x = x + 6;

        // From "Alpha" to the end of the following (blank) line.
        d.drag(&[(x, y), (x, y + 1)]);

        assert_eq!(d.clipboard_writes(), ["Alpha\n"]);
    }

    #[test]
    fn a_click_in_a_viewer_copies_nothing() {
        let mut d = open_done(DONE_FEED);
        let (x, y) = find(&d, "Old thing");

        d.click(x + 1, y);

        assert!(d.clipboard_writes().is_empty());
        assert!(shows(&d, "Done archive"));
    }

    #[test]
    fn the_file_view_follows_an_edit_on_disk_and_selects_the_new_text() {
        let mut d = open_file(DONE_FEED);
        std::fs::write(d.feed_path(), "# Work\n\n- [ ] Gamma ray\n").unwrap();
        d.push(AppEvent::FeedChanged);
        let (x, y) = find(&d, "- [ ] Gamma");

        d.drag(&[(x + 6, y), (x + 11, y)]);

        assert_eq!(d.clipboard_writes(), ["Gamma"]);
    }
}
