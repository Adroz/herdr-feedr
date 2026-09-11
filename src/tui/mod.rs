use crate::config::SidebarConfig;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use std::path::PathBuf;
use std::time::Duration;

pub fn run(_feed_path: PathBuf, _cfg: SidebarConfig) -> Result<()> {
    let mut terminal = ratatui::init();
    let res = (|| -> Result<()> {
        loop {
            terminal.draw(|f| {
                f.render_widget(
                    ratatui::text::Text::raw("feedr sidebar — q quits"),
                    f.area(),
                )
            })?;
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(k) = event::read()? {
                    if k.code == KeyCode::Char('q') {
                        return Ok(());
                    }
                }
            }
        }
    })();
    ratatui::restore();
    res
}
