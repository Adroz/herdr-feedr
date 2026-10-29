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
use crate::tui::socket::{FakeHerdr, PaneEntry};
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
/// The tab `PANE_ID` lives in.
pub const TAB_ID: &str = "w1:t1";

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
    panes: Rc<RefCell<Option<Vec<PaneEntry>>>>,
    exited: bool,
    _dir: tempfile::TempDir,
}

impl Driver {
    /// A sidebar of `w`x`h` cells over a temp feed holding `feed`.
    pub fn new(feed: &str, w: u16, h: u16) -> Self {
        Self::with_config(feed, w, h, |_| {})
    }

    /// As `new`, with the sidebar config adjusted by `tweak` first.
    pub fn with_config(feed: &str, w: u16, h: u16, tweak: impl FnOnce(&mut SidebarConfig)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.md");
        std::fs::write(&path, feed).unwrap();
        let mut cfg = SidebarConfig {
            side: Side::Left,
            width: 0.18,
            max_width: 46,
            auto_dock: false,
            auto_close: true,
            quit_herdr_when_last: true,
        };
        tweak(&mut cfg);
        let herdr = FakeHerdr::default();
        let herdr_log = herdr.log.clone();
        let panes = herdr.panes.clone();
        let clock = FakeClock::new(chrono::NaiveDate::from_ymd_opt(2026, 1, 15).unwrap());
        let clipboard = FakeClipboard::default();
        let mut app = App::new(path, cfg, Box::new(herdr));
        // Never inherit the developer's env: fixed pane, no $EDITOR.
        app.herdr_pane_id = Some(PANE_ID.into());
        app.editor_cmd = None;
        app.clock = Box::new(clock.clone());
        app.clipboard = Box::new(clipboard.clone());
        app.herdr_cli = Box::new(RecordingCli(herdr_log.clone()));
        // Short, and never from the env: a held lock fails fast in tests.
        app.lock_timeout = Duration::from_millis(50);
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
            panes,
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

    /// A bracketed terminal paste, delivered as one event.
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

    /// herdr pushes a `layout.updated` for `tab`, now holding `pane_ids`.
    pub fn layout(&mut self, tab: &str, pane_ids: &[&str]) {
        self.push(AppEvent::Layout {
            tab_id: tab.into(),
            pane_ids: pane_ids.iter().map(|p| p.to_string()).collect(),
        });
    }

    /// Set what herdr's `pane.list` answers from now on: `(pane, tab,
    /// title)` triples, all in workspace `w1`.
    pub fn set_panes(&mut self, panes: &[(&str, &str, &str)]) {
        *self.panes.borrow_mut() = Some(
            panes
                .iter()
                .map(|(p, t, title)| PaneEntry {
                    pane_id: p.to_string(),
                    tab_id: t.to_string(),
                    workspace_id: "w1".into(),
                    title: Some(title.to_string()),
                })
                .collect(),
        );
    }

    /// Make herdr's `pane.list` fail from now on.
    pub fn fail_pane_list(&mut self) {
        *self.panes.borrow_mut() = None;
    }

    /// Move the fake clock forward, then let the loop run once.
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

    /// The foreground colour drawn at one cell of the last frame.
    pub fn cell_fg(&self, x: u16, y: u16) -> ratatui::style::Color {
        self.terminal.backend().buffer()[(x, y)].fg
    }

    pub fn feed_path(&self) -> &Path {
        &self.app.feed_path
    }

    /// The feed file as it is on disk now.
    pub fn feed(&self) -> String {
        std::fs::read_to_string(&self.app.feed_path).unwrap()
    }

    /// The text of every cell drawn with the selection highlight, in reading
    /// order, rows joined by newlines (empty when nothing is highlighted).
    pub fn highlighted(&self) -> String {
        let bg = crate::tui::theme::selection().bg;
        let buf = self.terminal.backend().buffer();
        let area = buf.area;
        (0..area.height)
            .filter_map(|y| {
                let row: String = (0..area.width)
                    .filter(|&x| buf[(x, y)].style().bg == bg)
                    .map(|x| buf[(x, y)].symbol())
                    .collect();
                (!row.is_empty()).then_some(row)
            })
            .collect::<Vec<_>>()
            .join("\n")
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

    #[test]
    fn a_drag_across_a_soft_wrap_selects_what_was_dragged_over() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  one two three four five six seven\n");

        // From the 't' of "two" on row 1 to just after "sev" on the wrapped row 2.
        d.drag(&[(BODY_X + 4, BODY_Y), (BODY_X + 3, BODY_Y + 1)]);

        assert_eq!(d.clipboard_writes(), ["two three four five six sev"]);
    }

    #[test]
    fn a_drag_in_a_scrolled_body_selects_what_was_dragged_over() {
        let mut d = scrolled_body();

        // Top drawn row ("line4") down to the end of the third ("line6").
        d.drag(&[(BODY_X, BODY_Y), (BODY_X + 5, BODY_Y + 2)]);

        assert_eq!(d.clipboard_writes(), ["line4\nline5\nline6"]);
    }

    #[test]
    fn a_drag_after_wide_characters_selects_what_was_dragged_over() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  日本🙂abc\n");

        // From 本 (cell 2) to just after 'b' (cell 8).
        d.drag(&[(BODY_X + 2, BODY_Y), (BODY_X + 8, BODY_Y)]);

        assert_eq!(d.clipboard_writes(), ["本🙂ab"]);
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

        assert_eq!(d.clipboard_writes(), ["hello"]);
    }

    // --- Copy-on-select in edit fields (#33) --------------------------------

    fn status(d: &Driver) -> String {
        d.screen().last().unwrap().clone()
    }

    #[test]
    fn releasing_a_drag_copies_the_selection_and_reports_the_count() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  hello world\n");

        d.drag(&[(BODY_X, BODY_Y), (BODY_X + 5, BODY_Y)]);

        assert_eq!(d.clipboard_writes(), ["hello"]);
        assert!(status(&d).contains("Copied 5 chars"), "{:#?}", d.screen());
    }

    #[test]
    fn a_multi_line_body_drag_copies_with_newlines() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  one\n  two\n  three\n");

        // From the 'n' of "one" to just after "th" of "three".
        d.drag(&[(BODY_X + 1, BODY_Y), (BODY_X + 2, BODY_Y + 2)]);

        assert_eq!(d.clipboard_writes(), ["ne\ntwo\nth"]);
        assert!(status(&d).contains("Copied 9 chars"), "{:#?}", d.screen());
    }

    #[test]
    fn a_plain_click_or_zero_length_drag_copies_nothing() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  hello world\n");

        d.click(BODY_X + 2, BODY_Y);
        d.drag(&[(BODY_X + 3, BODY_Y), (BODY_X + 3, BODY_Y)]);

        assert!(d.clipboard_writes().is_empty());
        assert!(!status(&d).contains("Copied"), "{:#?}", d.screen());
    }

    #[test]
    fn ctrl_c_copies_the_selection_again() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  hello world\n");
        d.drag(&[(BODY_X + 6, BODY_Y), (BODY_X + 11, BODY_Y)]);

        d.key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert_eq!(d.clipboard_writes(), ["world", "world"]);
        assert!(status(&d).contains("Copied 5 chars"), "{:#?}", d.screen());
    }

    #[test]
    fn the_highlight_stays_after_copying_until_the_next_click() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  hello world\n");
        d.drag(&[(BODY_X, BODY_Y), (BODY_X + 5, BODY_Y)]);
        assert_eq!(d.highlighted(), "hello");
        d.key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(d.highlighted(), "hello", "Ctrl+C keeps it too");

        d.click(BODY_X + 8, BODY_Y);
        assert_eq!(d.highlighted(), "");
    }

    #[test]
    fn the_highlight_is_cleared_by_the_next_key() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  hello world\n");
        d.drag(&[(BODY_X, BODY_Y), (BODY_X + 5, BODY_Y)]);

        d.key(KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(d.highlighted(), "");
    }

    #[test]
    fn typing_after_a_copy_replaces_the_selection() {
        let mut d = open_first_item("# Feed\n\n- [ ] Alpha\n  hello world\n");
        d.drag(&[(BODY_X, BODY_Y), (BODY_X + 5, BODY_Y)]);

        d.type_str("bye");
        save(&mut d);

        assert!(d.feed().contains("  bye world\n"), "got: {}", d.feed());
    }

    #[test]
    fn dragging_in_the_sidebar_list_copies_nothing() {
        let mut d = Driver::new("# Feed\n\n- [ ] Alpha beta\n", 40, 12);

        d.drag(&[(6, 0), (12, 0)]);

        assert!(d.clipboard_writes().is_empty());
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
    fn a_save_after_an_agent_claims_the_item_keeps_the_edit_and_the_claim() {
        let mut d = Driver::new("# Feed\n\n- [ ] Alpha\n", 40, 24);
        d.click(6, 0);
        d.key(KeyCode::Tab, KeyModifiers::NONE); // Category -> Title
        d.type_str(" fixed");
        d.key(KeyCode::Tab, KeyModifiers::NONE); // Title -> Body
        d.type_str("notes");

        // An agent claims the item while the modal is open.
        std::fs::write(d.feed_path(), "# Feed\n\n- [~] Alpha @agent(claude:abc)\n").unwrap();
        d.push(AppEvent::FeedChanged);
        d.key(KeyCode::Char('s'), KeyModifiers::CONTROL);

        assert_eq!(
            d.feed(),
            "# Feed\n\n- [~] Alpha fixed @agent(claude:abc)\n  notes\n"
        );
        assert!(!d.screen().iter().any(|l| l.contains("Edit item")));
    }

    #[test]
    fn a_save_after_the_item_was_deleted_on_disk_says_so_and_keeps_the_modal() {
        let mut d = Driver::new("# Feed\n\n- [ ] Alpha\n- [ ] Beta\n", 40, 24);
        d.click(6, 0);
        d.key(KeyCode::Tab, KeyModifiers::NONE);
        d.type_str(" fixed");

        std::fs::write(d.feed_path(), "# Feed\n\n- [ ] Beta\n").unwrap();
        d.push(AppEvent::FeedChanged);
        d.key(KeyCode::Char('s'), KeyModifiers::CONTROL);

        assert_eq!(d.feed(), "# Feed\n\n- [ ] Beta\n", "nothing written");
        let screen = d.screen();
        assert!(
            screen.last().unwrap().contains("deleted on disk"),
            "status row reports the miss: {screen:#?}"
        );
        assert!(
            screen.iter().any(|l| l.contains("Alpha fixed")),
            "the typed edit stays in the open modal"
        );
    }

    // --- Today panel and modal toggle (#30) --------------------------------

    const TODAY_FEED: &str = "\
# Feed

## Work

- [ ] Fix auth @today
- [ ] Bump Node

## Later

- [ ] Evaluate pnpm
";

    #[test]
    fn today_items_sit_in_a_bordered_panel_above_the_list_not_in_their_section() {
        let d = Driver::new(TODAY_FEED, 24, 14);
        let screen = d.screen();
        assert_eq!(
            screen[..9],
            [
                "┌ Today ───────────────┐",
                "│[ ] Fix auth          │",
                "│    Work              │",
                "└──────────────────────┘",
                "",
                "Work",
                "[ ] Bump Node",
                "Later",
                "[ ] Evaluate pnpm",
            ],
            "{screen:#?}"
        );
        assert_eq!(
            screen.iter().filter(|l| l.contains("Fix auth")).count(),
            1,
            "a Today item is listed once, in the panel"
        );
    }

    #[test]
    fn the_today_panel_is_hidden_once_nothing_is_in_today() {
        let mut d = Driver::new(TODAY_FEED, 24, 14);
        std::fs::write(d.feed_path(), TODAY_FEED.replace(" @today", "")).unwrap();
        d.push(AppEvent::FeedChanged);

        let screen = d.screen();
        assert_eq!(screen[..3], ["Work", "[ ] Fix auth", "[ ] Bump Node"]);
        assert!(!screen.iter().any(|l| l.contains("Today")));
    }

    #[test]
    fn the_today_panel_outline_turns_red_when_a_hand_edit_overfills_it() {
        let red = crate::tui::theme::RED;
        let border_fg = |d: &Driver| d.cell_fg(0, 0);

        let full = "# Feed\n\n- [ ] A @today\n- [ ] B @today\n- [ ] C @today\n- [ ] D\n";
        let d = Driver::new(full, 24, 14);
        assert_ne!(border_fg(&d), red, "three is full, not over");

        let over = full.replace("- [ ] D", "- [ ] D @today");
        let d = Driver::new(&over, 24, 14);
        assert_eq!(border_fg(&d), red, "outline");
        assert_eq!(d.cell_fg(3, 0), red, "the ` Today ` title too");
        assert_eq!(d.screen()[0], "┌ Today ───────────────┐");
    }

    #[test]
    fn an_uncategorized_today_item_names_its_zone_heading_as_home() {
        let d = Driver::new("# Feed\n\n- [ ] Loose @today\n", 24, 14);
        assert_eq!(d.screen()[2], "│    Feed              │");
    }

    #[test]
    fn a_checkbox_click_in_the_today_panel_advances_the_item() {
        let mut d = Driver::new(TODAY_FEED, 24, 14);
        d.click(2, 1); // the state char of "│[ ] Fix auth"
        assert!(
            d.feed().contains("- [~] Fix auth @today\n"),
            "got: {}",
            d.feed()
        );
        assert_eq!(d.screen()[1], "│[~] Fix auth          │");
    }

    #[test]
    fn completing_a_today_item_strips_it_and_drops_it_from_the_panel() {
        let mut d = Driver::new(&TODAY_FEED.replace("[ ] Fix", "[~] Fix"), 24, 14);
        d.click(2, 1);
        assert!(d.feed().contains("- [x] Fix auth\n"), "got: {}", d.feed());
        let screen = d.screen();
        assert_eq!(screen[..2], ["Work", "[x] Fix auth"], "{screen:#?}");
    }

    #[test]
    fn a_title_click_in_the_today_panel_opens_its_edit_modal() {
        let mut d = Driver::new(TODAY_FEED, 40, 24);
        d.click(8, 1);
        let screen = d.screen();
        assert!(
            screen.iter().any(|l| l.contains("Edit item")),
            "{screen:#?}"
        );
        assert!(
            screen.iter().any(|l| l.contains("│Fix auth")),
            "{screen:#?}"
        );
    }

    #[test]
    fn an_agent_click_in_the_today_panel_resumes_its_session() {
        let feed = "# Feed\n\n## Work\n\n- [~] Fix auth @agent(claude:abc) @today\n";
        let mut d = Driver::new(feed, 24, 14);
        assert_eq!(d.screen()[3], "│  @claude             │");
        d.click(4, 3);
        assert!(
            d.herdr_calls().iter().any(|c| c.contains("abc")),
            "{:?}",
            d.herdr_calls()
        );
    }

    fn ctrl(d: &mut Driver, c: char) {
        d.key(KeyCode::Char(c), KeyModifiers::CONTROL);
    }

    /// The edit modal's top border row.
    fn modal_top(d: &Driver) -> String {
        d.screen()
            .into_iter()
            .find(|l| l.contains("Edit item"))
            .expect("the edit modal is open")
    }

    #[test]
    fn ctrl_t_marks_an_item_today_and_save_writes_the_token() {
        let mut d = Driver::new(TODAY_FEED, 40, 24);
        d.click(6, 6); // "[ ] Bump Node", under the panel and its gap row
        assert!(
            modal_top(&d).contains("Edit item ── ☆ Today"),
            "{}",
            modal_top(&d)
        );
        assert!(d.screen().iter().any(|l| l.contains("^T today")));

        ctrl(&mut d, 't');
        assert!(modal_top(&d).contains("★ Today"), "{}", modal_top(&d));
        save(&mut d);

        assert!(
            d.feed().contains("- [ ] Bump Node @today\n"),
            "got: {}",
            d.feed()
        );
        assert!(d.screen()[3].contains("[ ] Bump Node"), "{:#?}", d.screen());
    }

    #[test]
    fn ctrl_t_on_a_today_item_unmarks_it_and_save_strips_the_token() {
        let mut d = Driver::new(TODAY_FEED, 40, 24);
        d.click(8, 1); // "Fix auth" in the panel
        assert!(modal_top(&d).contains("★ Today"), "{}", modal_top(&d));

        ctrl(&mut d, 't');
        assert!(modal_top(&d).contains("☆ Today"), "{}", modal_top(&d));
        save(&mut d);

        assert!(d.feed().contains("- [ ] Fix auth\n"), "got: {}", d.feed());
        assert!(!d.screen().iter().any(|l| l.contains("Today")));
    }

    const FULL_TODAY: &str = "# Feed\n\n- [ ] A @today\n- [ ] B @today\n- [?] C @today\n- [ ] D\n";

    #[test]
    fn a_fourth_today_mark_is_refused_with_a_status_message() {
        let mut d = Driver::new(FULL_TODAY, 40, 24);
        d.click(6, 9); // "[ ] D", under the 8-row panel and its gap row
        let screen = d.screen();
        assert!(screen.iter().any(|l| l.contains("│D")), "{screen:#?}");

        ctrl(&mut d, 't');
        assert!(modal_top(&d).contains("☆ Today"), "the toggle stays unset");
        assert!(
            d.screen()
                .last()
                .unwrap()
                .contains("Today full — unmark one first"),
            "{:#?}",
            d.screen()
        );
        save(&mut d);
        assert_eq!(d.feed(), FULL_TODAY, "nothing to write");
    }

    #[test]
    fn a_today_mark_refused_at_save_keeps_the_modal_and_writes_nothing() {
        let feed = "# Feed\n\n- [ ] A @today\n- [ ] B @today\n- [ ] D\n- [ ] E\n";
        let mut d = Driver::new(feed, 40, 24);
        d.click(6, 7); // "[ ] D"
        ctrl(&mut d, 't');

        // Meanwhile a hand edit fills Today.
        let filled = feed.replace("- [ ] E", "- [ ] E @today");
        std::fs::write(d.feed_path(), &filled).unwrap();
        d.push(AppEvent::FeedChanged);
        save(&mut d);

        assert_eq!(d.feed(), filled, "nothing written");
        assert!(modal_top(&d).contains("☆ Today"), "{}", modal_top(&d));
        assert!(d
            .screen()
            .last()
            .unwrap()
            .contains("Today full — unmark one first"));
    }

    #[test]
    fn a_today_toggle_alone_makes_the_modal_dirty() {
        let mut d = Driver::new(TODAY_FEED, 40, 24);
        d.click(6, 6); // "[ ] Bump Node"
        ctrl(&mut d, 't');

        d.click(0, 0); // outside the modal panel
        assert!(
            modal_top(&d).contains("★ Today"),
            "a dirty modal stays open"
        );
        assert!(
            d.screen().last().unwrap().starts_with("unsaved"),
            "{:#?}",
            d.screen()
        );

        ctrl(&mut d, 't'); // toggled back: clean again
        d.click(0, 0);
        assert!(!d.screen().iter().any(|l| l.contains("Edit item")));
    }

    #[test]
    fn a_save_after_an_agent_claims_the_item_still_applies_the_today_toggle() {
        let mut d = Driver::new(TODAY_FEED, 40, 24);
        d.click(6, 6); // "[ ] Bump Node"
        ctrl(&mut d, 't');

        let claimed = TODAY_FEED.replace("- [ ] Bump Node", "- [~] Bump Node @agent(claude:abc)");
        std::fs::write(d.feed_path(), claimed).unwrap();
        d.push(AppEvent::FeedChanged);
        save(&mut d);

        assert!(
            d.feed()
                .contains("- [~] Bump Node @agent(claude:abc) @today\n"),
            "got: {}",
            d.feed()
        );
    }

    /// The panel at the narrowest and widest sidebar widths, with a claimed
    /// item and a long title.
    #[test]
    fn the_today_panel_at_the_sidebar_width_extremes() {
        let feed = "\
# Feed

## Work

- [ ] Fix the auth redirect loop on expired sessions @today
- [~] Migrate CI to GitHub Actions @agent(claude:abc) @today
- [ ] Bump Node

## Agent

- [?] Add retry to flaky deploy test @agent(claude:def) @today
";
        for w in [20, 46] {
            let d = Driver::new(feed, w, 18);
            insta::assert_snapshot!(format!("today_panel_{w}"), d.screen().join("\n"));
        }
    }

    // --- Render snapshots (#41) -------------------------------------------
    //
    // Text-only frames, reviewed with `cargo insta review` (see README).
    // Colour is invisible here: the over-cap red border is asserted by
    // `the_today_panel_outline_turns_red_when_a_hand_edit_overfills_it`.

    /// Four @today items: one over the cap, which only a hand edit can do.
    const OVER_CAP_FEED: &str = "\
# Feed

## Work

- [ ] Fix the auth redirect loop on expired sessions @today
- [~] Migrate CI to GitHub Actions @agent(claude:abc) @today
- [ ] Write the release notes for 0.4 @today
- [ ] Bump Node

## Agent

- [?] Add retry to flaky deploy test @agent(claude:def) @today
";

    #[test]
    fn the_today_panel_over_the_cap_grows_to_hold_every_item() {
        for w in [20, 46] {
            let d = Driver::new(OVER_CAP_FEED, w, 22);
            insta::assert_snapshot!(format!("today_panel_over_cap_{w}"), d.screen().join("\n"));
        }
    }

    #[test]
    fn with_nothing_in_today_the_panel_is_not_drawn() {
        let feed = "# Feed\n\n## Work\n\n- [ ] Fix the auth redirect loop on expired sessions\n- [ ] Bump Node\n";
        for w in [20, 46] {
            let d = Driver::new(feed, w, 12);
            insta::assert_snapshot!(format!("today_panel_empty_{w}"), d.screen().join("\n"));
        }
    }

    #[test]
    fn the_list_with_sections_bodies_and_agent_sub_lines() {
        let feed = "\
# Feed

- [ ] Uncategorised chore

## Work

- [ ] Fix the auth redirect loop on expired sessions
  Repro: log in, wait 30 minutes, reload.
- [~] Migrate CI to GitHub Actions @agent(claude:abc)
- [?] Pick a retry budget @agent(codex:xyz)

## Home

- [x] Book the plumber

# Done

- [x] Old thing @done(2026-01-10)
";
        let d = Driver::new(feed, 40, 20);
        insta::assert_snapshot!("list_sections_and_agents", d.screen().join("\n"));
    }

    #[test]
    fn the_edit_modal_for_a_today_item_shows_the_filled_star() {
        let mut d = Driver::new("# Feed\n\n## Work\n\n- [ ] Alpha @today\n", 40, 24);
        d.click(6, 1); // the item's row inside the Today panel
        assert!(d.screen().iter().any(|l| l.contains("Edit item")));
        insta::assert_snapshot!("edit_modal_today_on", d.screen().join("\n"));
    }

    #[test]
    fn the_done_archive_viewer() {
        let feed = "\
# Feed

- [ ] Alpha

# Done

## 2026-01-14

- [x] Ship the sidebar @done(2026-01-14)
- [x] Write the glossary @done(2026-01-14)
";
        let mut d = Driver::new(feed, 40, 16);
        d.click(0, 13); // "Done (n)" row, h-3
        assert!(d.screen().iter().any(|l| l.contains("Done archive")));
        insta::assert_snapshot!("done_archive", d.screen().join("\n"));
    }

    #[test]
    fn the_feed_file_viewer() {
        let feed = "# Feed\n\n## Work\n\n- [ ] Alpha @today\n- [ ] Beta\n";
        let mut d = Driver::new(feed, 40, 16);
        d.key(KeyCode::Char('f'), KeyModifiers::NONE);
        assert!(d.screen().iter().any(|l| l.contains("feed file")));
        insta::assert_snapshot!("file_view", d.screen().join("\n"));
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
    fn a_sidebar_write_waits_on_the_shared_feed_lock() {
        let mut d = Driver::new("# Feed\n\n- [x] Alpha\n", 100, 12);
        let before = d.feed();
        let held = std::fs::File::create(d.feed_path().with_file_name("feed.md.lock")).unwrap();
        held.lock().unwrap();

        d.click(2, 10); // "clear completed" while another writer holds the lock
        assert_eq!(d.feed(), before, "no write without the lock");
        assert!(
            d.screen().iter().any(|l| l.contains("feed.md.lock")),
            "the status line names the lock: {:#?}",
            d.screen()
        );

        drop(held);
        d.click(2, 10);
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
