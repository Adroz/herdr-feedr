//! Catppuccin Mocha palette + semantic style helpers for the sidebar TUI,
//! centralized here so a future config-driven theme can swap these without
//! touching render code (`view.rs` / `modal.rs` never spell out a raw
//! `Color::Rgb` themselves — they go through this module).
//!
//! Colors are `Color::Rgb` (herdr renders truecolor). On a terminal without
//! truecolor support these degrade via the terminal's own nearest-color
//! approximation rather than rendering exactly — acceptable per spec.

use crate::feed::State;
use crate::tui::socket::AgentStatus;
use ratatui::style::{Color, Modifier, Style};

/// Reserved for a future bordered (non-modal) panel — the sidebar's own
/// list/toolbar area has no `Block` border today, but the palette defines
/// this per spec so a config theme (or a later bordered-panel change) has
/// it ready without re-deriving the color.
#[allow(dead_code)]
pub const SURFACE1: Color = Color::Rgb(0x45, 0x47, 0x5a);
pub const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
pub const LAVENDER: Color = Color::Rgb(0xb4, 0xbe, 0xfe);
pub const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
pub const MAUVE: Color = Color::Rgb(0xcb, 0xa6, 0xf7);
pub const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
pub const OVERLAY1: Color = Color::Rgb(0x7f, 0x84, 0x9c);
pub const OVERLAY0: Color = Color::Rgb(0x6c, 0x70, 0x86);
pub const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);

/// Normal body text (item titles, viewer contents, "+ add").
pub fn normal_text() -> Style {
    Style::default().fg(TEXT)
}

/// Ordinary panel borders (unused directly by modals — see `modal_border`).
/// See `SURFACE1` — nothing renders a non-modal `Block` border yet.
#[allow(dead_code)]
pub fn border() -> Style {
    Style::default().fg(SURFACE1)
}

/// `##` section headers in the main list.
pub fn section_header() -> Style {
    Style::default().fg(LAVENDER).add_modifier(Modifier::BOLD)
}

/// Toolbar row, the bottom-pinned `Done (n)` row, and the status-message
/// row — all de-emphasized chrome, not content.
pub fn muted_row() -> Style {
    Style::default().fg(OVERLAY0)
}

/// `@agent` sub-line base color (overridden per-glyph by `agent_status`).
pub fn agent_subline() -> Style {
    Style::default().fg(OVERLAY1)
}

/// Item checkbox glyph color by state. Open uses the same color as normal
/// text (no special treatment); the others carry meaning.
pub fn item_glyph(state: State) -> Style {
    match state {
        State::Open => normal_text(),
        State::InProgress => Style::default().fg(YELLOW),
        State::Review => Style::default().fg(MAUVE),
        State::Done => Style::default().fg(GREEN).add_modifier(Modifier::DIM),
    }
}

/// Live agent-status glyph color on an `@agent` sub-line. `Unknown` never
/// renders a glyph at all (see `view::status_glyph`), so it has no color
/// here; `Idle` falls back to the sub-line's base color.
pub fn agent_status(status: AgentStatus) -> Style {
    match status {
        AgentStatus::Working => Style::default().fg(YELLOW),
        AgentStatus::Blocked => Style::default().fg(RED),
        AgentStatus::Done => Style::default().fg(GREEN),
        AgentStatus::Idle | AgentStatus::Unknown => agent_subline(),
    }
}

/// Modal and viewer chrome borders.
pub fn modal_border() -> Style {
    Style::default().fg(LAVENDER)
}

/// Edit/create modal `[ Save ]` / `[ Cancel ]` / `[ Delete ]` buttons —
/// reversed when focused (mouse click is the primary way to activate them;
/// Tab + Enter reaches them too, cheaply).
pub fn button(focused: bool) -> Style {
    if focused {
        Style::default().fg(TEXT).add_modifier(Modifier::REVERSED)
    } else {
        normal_text()
    }
}
