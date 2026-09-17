use ratatui::backend::{Backend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use ratatui::Terminal;

/// Draw the app into an in-memory buffer and return each row as a
/// right-trimmed string.
pub fn render_to_strings(app: &mut crate::tui::app::App, w: u16, h: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| crate::tui::view::draw(f, app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// Draw the app into an in-memory buffer and report where (if anywhere) the
/// real terminal cursor landed — `None` when the frame left it hidden (the
/// non-modal case, and any modal frame where no textarea has focus).
///
/// `TestBackend` doesn't expose a public "is the cursor visible" getter
/// (only `get_cursor_position`, which returns a position regardless of
/// visibility), so this reads its derived `Debug` output for `cursor:
/// true`/`false` — a fresh `TestBackend` per call, so a stale position from
/// a prior draw can never be mistaken for a currently-visible one.
pub fn render_cursor(app: &mut crate::tui::app::App, w: u16, h: u16) -> Option<Position> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| crate::tui::view::draw(f, app)).unwrap();
    let backend = terminal.backend_mut();
    let visible = format!("{backend:?}").contains("cursor: true");
    visible.then(|| backend.get_cursor_position().unwrap())
}

/// Draw the app into an in-memory buffer and return the raw `Buffer` — for
/// tests that need per-cell style (fg/bg/modifier), not just the rendered
/// text `render_to_strings` gives (round-3 item 1: dimmed-backdrop modal
/// overlay tests need to inspect `Modifier::DIM` and background color).
pub fn render_buffer(app: &mut crate::tui::app::App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| crate::tui::view::draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}
