use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Draw the app into an in-memory buffer and return each row as a
/// right-trimmed string.
pub fn render_to_strings(app: &crate::tui::app::App, w: u16, h: u16) -> Vec<String> {
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
