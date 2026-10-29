//! Headless sidebar test driver (spec #22, seam 1).
//!
//! Builds the sidebar over a temp feed with a fake herdr, a fake clipboard
//! and a fake clock, then drives it through the same `tui::step` the real
//! run loop uses. Tests feed terminal events, pushed herdr events and clock
//! advances in, and read back what a user (or herdr) could observe: the
//! screen, the feed file, clipboard writes, herdr calls and the exit request.

use crate::config::{Side, SidebarConfig};
use crate::tui::app::App;
use crate::tui::clipboard::FakeClipboard;
use crate::tui::clock::FakeClock;
use crate::tui::dock::Runner;
use crate::tui::socket::FakeHerdr;
use crate::tui::{step, AppEvent, Flow, Host};
use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

/// The pane id the driven sidebar believes it runs in.
pub const PANE_ID: &str = "w1:p1";

type Log = Rc<RefCell<Vec<String>>>;

/// Scripted terminal: hands out queued events one per step; runs no editor.
#[derive(Default)]
struct ScriptedHost {
    events: VecDeque<Event>,
}

impl Host<TestBackend> for ScriptedHost {
    fn next_event(&mut self, _timeout: Duration) -> Result<Option<Event>> {
        Ok(self.events.pop_front())
    }
    fn run_editor(&mut self, _: &mut Terminal<TestBackend>, _: &str, _: &Path) -> Result<()> {
        Ok(())
    }
}

/// `herdr` CLI fake that records into the shared herdr call log.
struct RecordingCli(Log);

impl Runner for RecordingCli {
    fn run(&mut self, args: &[&str]) -> Result<String> {
        self.0
            .borrow_mut()
            .push(format!("herdr {}", args.join(" ")));
        Ok(String::new())
    }
}

pub struct Driver {
    terminal: Terminal<TestBackend>,
    app: App,
    tx: mpsc::Sender<AppEvent>,
    rx: mpsc::Receiver<AppEvent>,
    host: ScriptedHost,
    clock: FakeClock,
    clipboard: FakeClipboard,
    herdr_log: Log,
    exited: bool,
    _dir: tempfile::TempDir,
}

impl Driver {
    /// A sidebar of `w`x`h` cells over a temp feed holding `feed`.
    pub fn new(feed: &str, w: u16, h: u16) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.md");
        std::fs::write(&path, feed).unwrap();
        let cfg = SidebarConfig {
            side: Side::Left,
            width: 0.18,
            max_width: 46,
            auto_dock: false,
        };
        let herdr = FakeHerdr::default();
        let herdr_log = herdr.log.clone();
        let clock = FakeClock::new(chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
        let clipboard = FakeClipboard::default();
        let mut app = App::new(path, cfg, Box::new(herdr));
        // Never inherit the developer's env: fixed pane, no $EDITOR.
        app.herdr_pane_id = Some(PANE_ID.into());
        app.editor_cmd = None;
        app.clock = Box::new(clock.clone());
        app.clipboard = Box::new(clipboard.clone());
        app.herdr_cli = Box::new(RecordingCli(herdr_log.clone()));
        app.reload();
        let (tx, rx) = mpsc::channel();
        let mut d = Driver {
            terminal: Terminal::new(TestBackend::new(w, h)).unwrap(),
            app,
            tx,
            rx,
            host: ScriptedHost::default(),
            clock,
            clipboard,
            herdr_log,
            exited: false,
            _dir: dir,
        };
        d.step();
        d
    }

    fn step(&mut self) {
        let flow = step(&mut self.terminal, &mut self.app, &self.rx, &mut self.host).unwrap();
        self.exited |= flow == Flow::Exit;
    }

    // --- In ----------------------------------------------------------------

    /// Feed one terminal event through a loop step.
    pub fn event(&mut self, ev: Event) {
        self.host.events.push_back(ev);
        self.step();
    }

    pub fn key(&mut self, code: KeyCode, mods: KeyModifiers) {
        self.event(Event::Key(KeyEvent::new(code, mods)));
    }

    pub fn type_str(&mut self, s: &str) {
        for c in s.chars() {
            self.key(KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    fn mouse(&mut self, kind: MouseEventKind, x: u16, y: u16) {
        self.event(Event::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }));
    }

    /// Left press then release at one cell.
    pub fn click(&mut self, x: u16, y: u16) {
        self.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
        self.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    }

    /// Left press on the first cell of `path`, drag through the rest, and
    /// release on the last.
    pub fn drag(&mut self, path: &[(u16, u16)]) {
        let (&(x0, y0), rest) = path.split_first().expect("a drag needs a start cell");
        self.mouse(MouseEventKind::Down(MouseButton::Left), x0, y0);
        for &(x, y) in rest {
            self.mouse(MouseEventKind::Drag(MouseButton::Left), x, y);
        }
        let &(x, y) = path.last().unwrap();
        self.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
    }

    // No consumer yet: paste and clock-driven behaviour land later in spec #22.
    #[allow(dead_code)]
    pub fn paste(&mut self, text: &str) {
        self.event(Event::Paste(text.into()));
    }

    /// Resize the terminal, as the tty would, then deliver the event.
    pub fn resize(&mut self, w: u16, h: u16) {
        self.terminal.backend_mut().resize(w, h);
        self.event(Event::Resize(w, h));
    }

    /// Push an event as the watcher or herdr subscriber would.
    pub fn push(&mut self, ev: AppEvent) {
        self.tx.send(ev).unwrap();
        self.step();
    }

    /// Move the fake clock forward, then let the loop run once.
    #[allow(dead_code)] // see `paste`
    pub fn advance(&mut self, by: Duration) {
        self.clock.advance(by);
        self.step();
    }

    // --- Out ---------------------------------------------------------------

    /// The last drawn frame, one right-trimmed string per row.
    pub fn screen(&self) -> Vec<String> {
        let buf = self.terminal.backend().buffer();
        let area = buf.area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    pub fn feed_path(&self) -> &Path {
        &self.app.feed_path
    }

    /// The feed file as it is on disk now.
    pub fn feed(&self) -> String {
        std::fs::read_to_string(&self.app.feed_path).unwrap()
    }

    pub fn clipboard_writes(&self) -> Vec<String> {
        self.clipboard.writes.borrow().clone()
    }

    /// Every herdr call made, socket and CLI alike, in order.
    pub fn herdr_calls(&self) -> Vec<String> {
        self.herdr_log.borrow().clone()
    }

    pub fn exit_requested(&self) -> bool {
        self.exited
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    fn click_an_item_type_and_ctrl_s_saves_the_edit_to_the_feed() {
        let mut d = Driver::new("# Feed\n\n- [ ] Alpha\n", 40, 24);
        assert!(d.screen().iter().any(|l| l.contains("[ ] Alpha")));

        d.click(6, 0); // title zone of the first row
        assert!(
            d.screen().iter().any(|l| l.contains("Edit item")),
            "clicking the title opens the edit modal"
        );
        insta::assert_snapshot!("edit_modal_open", d.screen().join("\n"));
        d.key(KeyCode::Tab, KeyModifiers::NONE); // Category -> Title
        d.type_str(" fixed");
        d.key(KeyCode::Char('s'), KeyModifiers::CONTROL);

        assert_eq!(d.feed(), "# Feed\n\n- [ ] Alpha fixed\n");
        assert!(d.screen().iter().any(|l| l.contains("[ ] Alpha fixed")));
        assert_eq!(
            d.herdr_calls(),
            ["zoom_pane w1:p1 on", "zoom_pane w1:p1 off"],
            "opening and closing the modal zooms and unzooms our pane"
        );
        assert!(d.clipboard_writes().is_empty());
        assert!(!d.exit_requested());

        d.key(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(d.exit_requested());
    }

    // --- Click/drag positions in edit fields (#32) --------------------------
    //
    // At 40x24 the edit modal's Body field draws its text in columns 5..=34
    // (30 cells) from row 11 down (6 rows) (see the `edit_modal_open` snapshot). Each
    // test clicks a cell, types a marker and saves, so the feed shows exactly
    // which logical character the click landed on.

    const BODY_X: u16 = 5;
    const BODY_Y: u16 = 11;

    /// Open the edit modal for the first item, as a user would.
    fn open_first_item(feed: &str) -> Driver {
        let mut d = Driver::new(feed, 40, 24);
        d.click(6, 0);
        assert!(d.screen().iter().any(|l| l.contains("Edit item")));
        d
    }

    fn save(d: &mut Driver) {
        d.key(KeyCode::Char('s'), KeyModifiers::CONTROL);
    }

    #[test]
    fn a_click_on_a_wrapped_continuation_row_lands_under_the_pointer() {
        // 33 chars: word-wraps after "six " so "seven" starts row 2.
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  one two three four five six seven\n");
        assert!(
            d.screen()[BODY_Y as usize + 1].contains("│seven"),
            "{:#?}",
            d.screen()
        );

        d.click(BODY_X + 2, BODY_Y + 1); // the 'v' of "seven"
        d.type_str("X");
        save(&mut d);

        assert!(
            d.feed().contains("  one two three four five six seXven\n"),
            "got: {}",
            d.feed()
        );
    }

    /// Ten body lines in a six-row field: Tab into Body parks the cursor at
    /// the end, which scrolls the field so "line4".."line9" are drawn.
    fn scrolled_body() -> Driver {
        let body: String = (0..10).map(|i| format!("  line{i}\n")).collect();
        let mut d = open_first_item(&format!("# Feed\n\n- [ ] Alpha\n{body}"));
        d.key(KeyCode::Tab, KeyModifiers::NONE); // Category -> Title
        d.key(KeyCode::Tab, KeyModifiers::NONE); // Title -> Body, cursor at end
        assert!(
            d.screen()[BODY_Y as usize].contains("│line4"),
            "{:#?}",
            d.screen()
        );
        d
    }

    #[test]
    fn a_click_in_a_scrolled_body_lands_under_the_pointer() {
        let mut d = scrolled_body();

        d.click(BODY_X + 1, BODY_Y); // the 'i' of the top drawn row, "line4"
        assert!(
            d.screen()[BODY_Y as usize].contains("│line4"),
            "the click must not scroll the field away from what was clicked: {:#?}",
            d.screen()
        );
        d.type_str("X");
        save(&mut d);

        assert!(d.feed().contains("  lXine4\n"), "got: {}", d.feed());
    }

    #[test]
    fn a_click_after_wide_characters_lands_under_the_pointer() {
        // 日, 本 and 🙂 each draw two cells wide, so 'a' is in cell 6.
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  日本🙂ab\n");

        d.click(BODY_X + 6, BODY_Y);
        d.type_str("X");
        save(&mut d);

        assert!(d.feed().contains("  日本🙂Xab\n"), "got: {}", d.feed());
    }

    #[test]
    fn a_click_on_the_right_half_of_a_wide_character_lands_on_it() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  日本🙂ab\n");

        d.click(BODY_X + 3, BODY_Y); // second cell of 本
        d.type_str("X");
        save(&mut d);

        assert!(d.feed().contains("  日X本🙂ab\n"), "got: {}", d.feed());
    }

    fn copy(d: &mut Driver) -> Vec<String> {
        d.key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        d.clipboard_writes()
    }

    #[test]
    fn a_drag_across_a_soft_wrap_selects_what_was_dragged_over() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  one two three four five six seven\n");

        // From the 't' of "two" on row 1 to just after "sev" on the wrapped row 2.
        d.drag(&[(BODY_X + 4, BODY_Y), (BODY_X + 3, BODY_Y + 1)]);

        assert_eq!(copy(&mut d), ["two three four five six sev"]);
    }

    #[test]
    fn a_drag_in_a_scrolled_body_selects_what_was_dragged_over() {
        let mut d = scrolled_body();

        // Top drawn row ("line4") down to the end of the third ("line6").
        d.drag(&[(BODY_X, BODY_Y), (BODY_X + 5, BODY_Y + 2)]);

        assert_eq!(copy(&mut d), ["line4\nline5\nline6"]);
    }

    #[test]
    fn a_drag_after_wide_characters_selects_what_was_dragged_over() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  日本🙂abc\n");

        // From 本 (cell 2) to just after 'b' (cell 8).
        d.drag(&[(BODY_X + 2, BODY_Y), (BODY_X + 8, BODY_Y)]);

        assert_eq!(copy(&mut d), ["本🙂ab"]);
    }

    #[test]
    fn a_drag_that_leaves_the_field_keeps_the_last_in_field_endpoint() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  hello world\n");

        // Over "hello", then up into the Title field: ignored, not redirected.
        d.drag(&[
            (BODY_X, BODY_Y),
            (BODY_X + 5, BODY_Y),
            (BODY_X + 2, BODY_Y - 3),
        ]);

        assert_eq!(copy(&mut d), ["hello"]);
    }

    #[test]
    fn a_click_past_the_end_of_a_line_lands_at_its_end() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  hello\n");

        d.click(BODY_X + 20, BODY_Y);
        d.type_str("X");
        save(&mut d);

        assert!(d.feed().contains("  helloX\n"), "got: {}", d.feed());
    }

    #[test]
    fn a_click_in_the_title_lands_under_the_pointer() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha beta\n");

        d.click(BODY_X + 6, 8); // Title text row; the 'b' of "beta"
        d.type_str("X");
        save(&mut d);

        assert_eq!(d.feed(), "# Feed\n\n- [ ] Alpha Xbeta\n");
    }

    #[test]
    fn a_pushed_feed_change_redraws_from_disk() {
        let mut d = Driver::new("# Feed\n\n- [ ] Alpha\n", 40, 12);
        std::fs::write(d.feed_path(), "# Feed\n\n- [ ] Beta\n").unwrap();
        d.push(AppEvent::FeedChanged);
        assert!(d.screen().iter().any(|l| l.contains("[ ] Beta")));
        assert!(!d.screen().iter().any(|l| l.contains("Alpha")));
    }

    #[test]
    fn resize_redraws_at_the_new_size() {
        let mut d = Driver::new("# Feed\n\n- [ ] Alpha\n", 40, 12);
        d.resize(30, 8);
        let screen = d.screen();
        assert_eq!(screen.len(), 8);
        assert!(screen.iter().all(|l| l.chars().count() <= 30));
        assert!(screen[0].contains("[ ] Alpha"));
    }

    #[test]
    fn sweep_stamps_the_injected_clock_date() {
        let mut d = Driver::new("# Feed\n\n- [x] Alpha\n", 40, 12);
        d.click(2, 10); // "clear completed" on the toolbar (row h-2)
        assert!(d.feed().contains("@done(2026-01-15)"), "got: {}", d.feed());
    }

    #[test]
    fn collapse_resize_goes_through_the_recorded_herdr_cli() {
        let mut d = Driver::new("# Feed\n\n- [ ] Alpha\n", 40, 12);
        d.click(39, 11); // collapse chevron, status row (h-1)
        assert_eq!(
            d.herdr_calls(),
            ["herdr pane resize --direction left --amount 0.18 --pane w1:p1"]
        );
    }
}
